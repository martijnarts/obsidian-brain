//! `delete_folder`: remove a folder from disk. Every note under it goes
//! through `delete_note` first, so its node, edges, embedding and orphaned
//! stubs leave the index in the same call. Indexed notes under the folder
//! that are already gone from disk are purged too.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Result, anyhow, bail};
use rusqlite::Connection;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::args;
use crate::mcp::tool::ToolSpec;
use crate::tools::support::background_reindex::run_background_reindex;
use crate::util::errors::node_fs_error_message;
use crate::vault::mover::delete_note;
use crate::vault::vault_path::resolve_vault_path;

/// How many paths a dry run lists before it truncates.
const DRY_RUN_SAMPLE: usize = 50;

const DESCRIPTION: &str = "Permanently delete a folder. Refuses a non-empty folder unless `recursive: true`; notes inside also leave the index. Requires `confirm: true`; `dryRun` lists what would go.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Vault-relative folder path.
    path: String,
    /// Must literally be `true` to execute. Guards against accidental deletion.
    #[schemars(extend("const" = true))]
    #[allow(dead_code)]
    confirm: bool,
    /// Delete the folder with everything in it. Default false: refuse a non-empty folder.
    recursive: Option<bool>,
    /// If true, report what would be deleted without removing anything.
    dry_run: Option<bool>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("delete_folder", DESCRIPTION, |ctx, args| async move {
        let resolved = resolve_vault_path(&ctx.config.vault_path, &args.path)?;
        let (rel, abs) = (resolved.rel, resolved.abs);
        if rel.is_empty() {
            bail!("Refusing to delete the vault root");
        }
        if rel == ".obsidian" || rel.starts_with(".obsidian/") {
            bail!("Refusing to delete the .obsidian config folder");
        }
        let st =
            std::fs::symlink_metadata(&abs).map_err(|_| anyhow!("Folder not found: \"{rel}\""))?;
        if st.file_type().is_symlink() {
            bail!("{rel} is a symlink, not a folder");
        }
        if !st.is_dir() {
            bail!("{rel} is a file, not a folder: use delete_note");
        }

        let mut files = Vec::new();
        let mut folders = Vec::new();
        walk(&abs, &rel, &mut files, &mut folders)?;
        let indexed = ctx.db.with(|conn| indexed_notes_under(conn, &rel))?;
        let mut seen = HashSet::new();
        let notes: Vec<String> = files
            .iter()
            .filter(|f| f.to_lowercase().ends_with(".md"))
            .cloned()
            .chain(indexed)
            .filter(|n| seen.insert(n.clone()))
            .collect();
        let empty = files.is_empty() && folders.is_empty();
        let refusal = (!empty && args.recursive != Some(true)).then(|| {
            format!(
                "Folder {rel} is not empty ({} files, {} folders). Pass recursive: true to delete it with its contents.",
                files.len(),
                folders.len()
            )
        });

        if args.dry_run == Some(true) {
            let mut paths: Vec<&String> = folders.iter().chain(&files).collect();
            paths.sort_by(|a, b| args::utf16_cmp(a, b));
            let mut out = Map::new();
            out.insert("dryRun".into(), true.into());
            out.insert("path".into(), rel.clone().into());
            out.insert("files".into(), files.len().into());
            out.insert("folders".into(), folders.len().into());
            out.insert("notes".into(), notes.len().into());
            out.insert(
                "sample".into(),
                json!(paths.iter().take(DRY_RUN_SAMPLE).collect::<Vec<_>>()),
            );
            out.insert("truncated".into(), (paths.len() > DRY_RUN_SAMPLE).into());
            if let Some(refused) = refusal {
                out.insert("refused".into(), refused.into());
            }
            return Ok(Value::Object(out));
        }
        if let Some(refused) = refusal {
            bail!(refused);
        }

        let vault = ctx.config.vault_path.clone();
        let (nodes, edges, stubs_pruned) = ctx.db.with(|conn| {
            let (mut nodes, mut edges, mut stubs) = (0usize, 0i64, 0usize);
            for note in &notes {
                let result = delete_note(&vault, note, conn)?;
                if result.deleted_from_index.node {
                    nodes += 1;
                }
                edges += result.deleted_from_index.edges;
                stubs += result.deleted_from_index.stubs_pruned;
            }
            anyhow::Ok((nodes, edges, stubs))
        })?;
        if empty {
            std::fs::remove_dir(&abs)
                .map_err(|e| anyhow!(node_fs_error_message(&e, "rmdir", &abs, None)))?;
        } else {
            std::fs::remove_dir_all(&abs)
                .map_err(|e| anyhow!(node_fs_error_message(&e, "rm", &abs, None)))?;
        }

        // The notes left the index above; the reindex re-resolves the links
        // that pointed at them, which become stubs.
        run_background_reindex(&ctx);

        Ok(json!({
            "path": rel,
            "deleted": { "files": files.len(), "folders": folders.len(), "notes": notes.len() },
            "deletedFromIndex": { "nodes": nodes, "edges": edges, "stubsPruned": stubs_pruned },
        }))
    })
}

/// Every file and folder under `abs`, as vault-relative paths. Symlinks
/// count as files and are not followed.
fn walk(abs: &Path, rel: &str, files: &mut Vec<String>, folders: &mut Vec<String>) -> Result<()> {
    let entries = std::fs::read_dir(abs)
        .map_err(|e| anyhow!(node_fs_error_message(&e, "scandir", abs, None)))?;
    for entry in entries {
        let entry = entry?;
        let child_rel = format!("{rel}/{}", entry.file_name().to_string_lossy());
        if entry.file_type()?.is_dir() {
            folders.push(child_rel.clone());
            walk(&entry.path(), &child_rel, files, folders)?;
        } else {
            files.push(child_rel);
        }
    }
    Ok(())
}

fn indexed_notes_under(conn: &Connection, rel: &str) -> Result<Vec<String>> {
    let prefix = format!("{rel}/");
    let mut stmt = conn.prepare("SELECT id FROM nodes WHERE substr(id, 1, ?) = ?")?;
    let rows = stmt.query_map(
        rusqlite::params![prefix.chars().count() as i64, prefix],
        |r| r.get::<_, String>(0),
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}
