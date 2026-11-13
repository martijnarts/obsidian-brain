//! `list_tasks`: markdown tasks read from the files on disk, so a task
//! ticked a moment ago shows its new state before the index catches up.

use std::path::Path;

use anyhow::{Result, bail};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::mcp::tool::ToolSpec;
use crate::resolve::single_note::resolve_single_note;
use crate::vault::parser::collect_markdown_files;
use crate::vault::tasks::{TaskState, scan_tasks};
use crate::vault::vault_path::{normalize_folder, read_note_file, resolve_folder};

const DESCRIPTION: &str = "List markdown tasks (`- [ ] text`) in the vault, one folder, or one note, ordered by path and line. Each task has its 1-based `line`, raw `status` character, `state` (open/done/cancelled), `text`, nesting `indent`, `parentLine` when nested under another task, and `due` from `📅 YYYY-MM-DD`. Use `line` with `set_task_status` or `ensure_block_id`.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Wanted {
    Open,
    Done,
    All,
}

fn limit<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    use serde::de::Error;
    match Option::<u64>::deserialize(d)? {
        Some(n) if !(1..=1000).contains(&n) => Err(D::Error::custom(format!(
            "expected an integer from 1 to 1000, got {n}"
        ))),
        other => Ok(other),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Path or fuzzy match of one note. Omit to scan the vault.
    name: Option<String>,
    /// Vault-relative folder to scan recursively.
    folder: Option<String>,
    /// Default `open`: every status but `x`, `X` and `-`. `done`: `x`/`X`. `all` adds cancelled (`-`).
    status: Option<Wanted>,
    /// Max tasks to return. Default 200, max 1000.
    #[serde(default, deserialize_with = "limit")]
    #[schemars(range(min = 1, max = 1000))]
    limit: Option<u64>,
    /// Tasks to skip, for paging. Default 0.
    offset: Option<u64>,
}

/// JS's default sort order: by UTF-16 code units.
fn utf16_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

fn notes_under(vault: &Path, folder: &str) -> Result<Vec<String>> {
    let rel = normalize_folder(Some(folder))?;
    if rel.split('/').any(|seg| seg.starts_with('.')) {
        bail!("Hidden folders are not scanned: \"{folder}\"");
    }
    resolve_folder(vault, Some(folder))?;
    let mut paths = collect_markdown_files(vault, &rel)?;
    paths.sort_by(|a, b| utf16_cmp(a, b));
    Ok(paths)
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("list_tasks", DESCRIPTION, |ctx, args| async move {
        if args.name.is_some() && args.folder.is_some() {
            bail!("Pass `name` or `folder`, not both.");
        }
        let wanted = args.status.unwrap_or(Wanted::Open);
        let limit = args.limit.unwrap_or(200) as usize;
        let offset = args.offset.unwrap_or(0) as usize;
        let vault = ctx.config.vault_path.clone();

        let mut notes: Vec<(String, String)> = Vec::new();
        if let Some(name) = args.name {
            let id = ctx
                .db
                .run(move |c| resolve_single_note(&name, c, false))
                .await?;
            let note = read_note_file(&vault, &id)?;
            notes.push((note.rel, note.content));
        } else {
            for path in notes_under(&vault, args.folder.as_deref().unwrap_or(""))? {
                // A symlink out of the vault, or a file deleted mid-scan, is skipped.
                if let Ok(note) = read_note_file(&vault, &path) {
                    notes.push((path, note.content));
                }
            }
        }

        let mut matched: Vec<Value> = Vec::new();
        for (path, content) in &notes {
            for task in scan_tasks(content) {
                let keep = match wanted {
                    Wanted::All => true,
                    Wanted::Open => task.state == TaskState::Open,
                    Wanted::Done => task.state == TaskState::Done,
                };
                if !keep {
                    continue;
                }
                let mut entry = Map::new();
                entry.insert("path".into(), path.clone().into());
                if let Value::Object(fields) = serde_json::to_value(&task)? {
                    entry.extend(fields);
                }
                matched.push(Value::Object(entry));
            }
        }

        let total = matched.len();
        let tasks: Vec<Value> = matched.into_iter().skip(offset).take(limit).collect();
        Ok(json!({
            "total": total,
            "offset": offset,
            "truncated": offset + tasks.len() < total,
            "tasks": tasks,
        }))
    })
}
