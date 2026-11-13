//! `vault_overview`: a one-call snapshot of an unfamiliar vault.

use std::path::Path;

use anyhow::{Result, anyhow};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::mcp::tool::ToolSpec;
use crate::store::nodes::all_notes;
use crate::store::sync::all_sync_mtimes;
use crate::util::errors::node_fs_error_message;
use crate::util::js;
use crate::util::js::locale_compare;
use crate::vault::tags::{count_tags, sort_tag_counts};

const DESCRIPTION: &str = "One-call orientation for an unfamiliar vault: note and attachment counts, notes per top-level folder, the most used tags, and the most recently modified notes. Use `list_tags` or `list_notes` for more than the snapshot.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Tags to include. Default 15.
    #[schemars(range(min = 1))]
    top_tags: Option<u64>,
    /// Recently modified notes to include. Default 10.
    #[schemars(range(min = 1))]
    recent: Option<u64>,
}

/// Non-Markdown files on disk, skipping hidden entries like `.obsidian/`.
fn count_attachments(dir: &Path) -> Result<usize> {
    let mut n = 0;
    let entries = std::fs::read_dir(dir)
        .map_err(|e| anyhow!(node_fs_error_message(&e, "scandir", dir, None)))?;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            n += count_attachments(&entry.path())?;
        } else if kind.is_file() && !name.ends_with(".md") {
            n += 1;
        }
    }
    Ok(n)
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("vault_overview", DESCRIPTION, |ctx, args| async move {
        let (notes, mtimes) = ctx
            .db
            .with(|conn| anyhow::Ok((all_notes(conn)?, all_sync_mtimes(conn)?)))?;

        let mut folder_counts: Vec<(String, usize)> = Vec::new();
        for n in &notes {
            let folder = match n.id.find('/') {
                Some(slash) => &n.id[..slash],
                None => "(root)",
            };
            match folder_counts.iter_mut().find(|(f, _)| f == folder) {
                Some((_, count)) => *count += 1,
                None => folder_counts.push((folder.to_string(), 1)),
            }
        }
        folder_counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| locale_compare(&a.0, &b.0)));
        let folders: Vec<_> = folder_counts
            .into_iter()
            .map(|(folder, count)| json!({ "folder": folder, "count": count }))
            .collect();

        let top_tags: Vec<_> =
            sort_tag_counts(count_tags(notes.iter().map(|n| &n.frontmatter), true))
                .into_iter()
                .take(args.top_tags.unwrap_or(15) as usize)
                .collect();

        let mtime = |id: &str| mtimes.get(id).copied().unwrap_or(0.0);
        let mut recent: Vec<_> = notes.iter().filter(|n| mtime(&n.id) > 0.0).collect();
        recent.sort_by(|a, b| {
            mtime(&b.id)
                .total_cmp(&mtime(&a.id))
                .then_with(|| locale_compare(&a.id, &b.id))
        });
        let recent: Vec<_> = recent
            .into_iter()
            .take(args.recent.unwrap_or(10) as usize)
            .map(|n| {
                json!({ "id": n.id, "title": n.title, "mtime": js::iso_from_ms(mtime(&n.id)).unwrap_or_default() })
            })
            .collect();

        Ok(json!({
            "notes": notes.len(),
            "attachments": count_attachments(&ctx.config.vault_path)?,
            "folders": folders,
            "topTags": top_tags,
            "recent": recent,
        }))
    })
}
