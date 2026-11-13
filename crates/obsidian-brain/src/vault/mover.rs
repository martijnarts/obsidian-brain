//! Move, rename and delete `.md` files inside the vault.
//!
//! [`move_note`] only touches the file being moved. Rewriting inbound
//! wiki-links on a move is the `move_note` tool's job, driven by the edge
//! store.

use std::io;
use std::path::Path;

use anyhow::{Result, anyhow, bail};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

use super::frontmatter::{self, FrontmatterOp, edit_frontmatter};
use super::vault_path::{read_utf8, resolve_vault_path, write_file_atomic};
use crate::store::edges::{count_edges_by_source, delete_edges_by_source, get_edges_by_source};
use crate::store::embeddings::delete_embedding;
use crate::store::nodes::{delete_node, prune_orphan_stubs};
use crate::store::sync::delete_sync_path;
use crate::util::errors::node_fs_error_message;
use crate::util::js::basename_md;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveResult {
    /// Vault-relative source path.
    pub old_path: String,
    /// Vault-relative destination path.
    pub new_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedFromIndex {
    pub node: bool,
    pub edges: i64,
    pub embedding: bool,
    pub stubs_pruned: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteResult {
    /// The vault-relative path that was asked for.
    pub path: String,
    pub deleted_from_index: DeletedFromIndex,
}

/// Rename or move a note.
///
/// `destination_rel` is read flexibly:
///   - A trailing `/` makes it a directory; the source file name is kept.
///   - A last segment without a dot gets `.md` appended.
///   - Anything else is a full vault-relative file path.
///
/// Intermediate directories are created. The index is not touched; callers
/// trigger a re-scan afterwards.
pub fn move_note(vault_path: &Path, source_rel: &str, destination_rel: &str) -> Result<MoveResult> {
    let resolved_source = resolve_vault_path(vault_path, source_rel)?.abs;

    // A plain rename would surface a cryptic ENOENT.
    if std::fs::metadata(&resolved_source).is_err() {
        bail!("moveNote: source not found: {source_rel}");
    }

    // The destination comes from the client: it must stay inside the vault.
    let dest = resolve_vault_path(
        vault_path,
        &normalize_destination(destination_rel, source_rel),
    )?;
    if dest.abs == resolved_source {
        return Ok(MoveResult {
            old_path: source_rel.to_string(),
            new_path: dest.rel,
        });
    }

    if let Some(parent) = dest.abs.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!(node_fs_error_message(&e, "mkdir", parent, None)))?;
    }
    std::fs::rename(&resolved_source, &dest.abs).map_err(|e| {
        anyhow!(node_fs_error_message(
            &e,
            "rename",
            &resolved_source,
            Some(&dest.abs)
        ))
    })?;

    // Keep `title` in step with the file name when it tracked the old
    // basename. A custom title stays, and a missing one is not added. A
    // failure here never fails the move.
    if dest.rel.to_lowercase().ends_with(".md") {
        let _ = sync_title_to_filename(&dest.abs, source_rel, &dest.rel);
    }

    Ok(MoveResult {
        old_path: source_rel.to_string(),
        new_path: dest.rel,
    })
}

/// If the moved file's frontmatter `title` equals the old basename, set it
/// to the new basename. Only the `title` line changes.
fn sync_title_to_filename(abs_dest: &Path, old_rel: &str, new_rel: &str) -> Result<()> {
    let raw = read_utf8(abs_dest)?;
    let Ok(parsed) = frontmatter::parse(&raw) else {
        return Ok(());
    };
    let Some(Value::String(title)) = parsed.data.get("title") else {
        return Ok(());
    };
    let new_base = basename_md(new_rel);
    if title != basename_md(old_rel) {
        return Ok(());
    }
    let edited = edit_frontmatter(
        &raw,
        &[FrontmatterOp::Set(
            "title".into(),
            Value::String(new_base.to_string()),
        )],
    )?;
    write_file_atomic(abs_dest, edited.text)?;
    Ok(())
}

/// Turn a caller-supplied destination into a vault-relative file path.
fn normalize_destination(dest: &str, source_rel: &str) -> String {
    let source_file = source_rel.rsplit(['/', '\\']).next().unwrap_or(source_rel);

    if dest.ends_with('/') || dest.ends_with('\\') {
        let dir = dest.trim_end_matches(['/', '\\']);
        return if dir.is_empty() || dir == "." {
            source_file.to_string()
        } else {
            format!("{dir}/{source_file}")
        };
    }

    // Without file system information, a last segment without a dot is a
    // file name without its extension.
    let last = dest.rsplit(['/', '\\']).next().unwrap_or("");
    if !last.contains('.') {
        return format!("{dest}.md");
    }

    // Another extension is honoured as given; the tool layer validates it.
    dest.to_string()
}

/// Delete a note and its index entries.
///
/// Order: edges, stubs left orphaned, embedding, node, sync path, then the
/// file. If the file is already gone the index is still reconciled. The
/// edge count is read before the edges go.
pub fn delete_note(
    vault_path: &Path,
    file_rel_path: &str,
    conn: &Connection,
) -> Result<DeleteResult> {
    let abs = resolve_vault_path(vault_path, file_rel_path)?.abs;

    let edge_count = count_edges_by_source(conn, file_rel_path).unwrap_or(0);

    // Outbound stub targets, read before the edges go, so stubs this note
    // was the last to reference can be pruned.
    let stub_targets: Vec<String> = get_edges_by_source(conn, file_rel_path)
        .map(|edges| {
            edges
                .into_iter()
                .map(|e| e.target_id)
                .filter(|id| id.starts_with("_stub/"))
                .collect()
        })
        .unwrap_or_default();

    let mut node_deleted = false;
    let mut embedding_deleted = false;
    let mut stubs_pruned = 0;
    match delete_edges_by_source(conn, file_rel_path) {
        Ok(()) => {
            stubs_pruned = prune_orphan_stubs(conn, &stub_targets).unwrap_or(0);
            embedding_deleted = delete_embedding(conn, file_rel_path).is_ok();
            node_deleted = delete_node(conn, file_rel_path).is_ok();
            // The sync table may not know this path; that is fine.
            let _ = delete_sync_path(conn, file_rel_path);
        }
        Err(err) => {
            // The unlink below still runs; the result reports what happened.
            tracing::warn!("deleteNote: index cleanup failed for {file_rel_path}: {err}");
        }
    }

    match std::fs::remove_file(&abs) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => bail!(node_fs_error_message(&e, "unlink", &abs, None)),
    }

    Ok(DeleteResult {
        path: file_rel_path.to_string(),
        deleted_from_index: DeletedFromIndex {
            node: node_deleted,
            edges: edge_count,
            embedding: embedding_deleted,
            stubs_pruned,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_destination_rules() {
        assert_eq!(
            normalize_destination("Archive/", "Notes/a.md"),
            "Archive/a.md"
        );
        assert_eq!(normalize_destination("/", "Notes/a.md"), "a.md");
        assert_eq!(
            normalize_destination("Notes/renamed", "a.md"),
            "Notes/renamed.md"
        );
        assert_eq!(normalize_destination("x/pic.png", "a.md"), "x/pic.png");
        assert_eq!(basename_md("a/b.md"), "b");
        assert_eq!(basename_md(".md"), ".md");
    }
}
