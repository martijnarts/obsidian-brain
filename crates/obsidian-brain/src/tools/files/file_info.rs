//! `file_info`: describe a vault path without returning its body. Sizes and
//! times come from the file system; heading, task and tag counts from the
//! note on disk; link counts from the index, so they lag a fresh write until
//! the background reindex catches up.

use std::collections::HashSet;
use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::anyhow;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::mcp::tool::ToolSpec;
use crate::store::edges::{count_edges_by_target, get_edges_by_source};
use crate::store::nodes::get_node;
use crate::types::Frontmatter;
use crate::util::errors::node_fs_error_message;
use crate::util::js;
use crate::util::js::iso_time;
use crate::vault::frontmatter;
use crate::vault::note_parts::{count_tasks, extract_headings};
use crate::vault::parser::extract_inline_tags;
use crate::vault::vault_path::{read_utf8, resolve_vault_path};

const DESCRIPTION: &str = "Describe a vault path without its body: kind (note, attachment, folder), size, times, and for a note its link, heading, task and tag counts. Link counts come from the index.";

#[derive(Debug, Deserialize, JsonSchema)]
struct Args {
    /// Vault-relative path of a note, attachment or folder.
    path: String,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("file_info", DESCRIPTION, |ctx, args| async move {
        let resolved = resolve_vault_path(&ctx.config.vault_path, &args.path)?;
        let (rel, abs) = (resolved.rel, resolved.abs);
        let st = std::fs::metadata(&abs).map_err(|_| anyhow!("Path not found: {}", args.path))?;
        let mtime = iso_time(st.modified().unwrap_or(UNIX_EPOCH));
        let ctime = iso_time(birth_or_change_time(&st));

        if st.is_dir() {
            let children = std::fs::read_dir(&abs)
                .map_err(|e| anyhow!(node_fs_error_message(&e, "scandir", &abs, None)))?
                .count();
            return Ok(json!({
                "path": rel,
                "kind": "folder",
                "children": children,
                "mtime": mtime,
                "ctime": ctime,
            }));
        }
        if !rel.to_lowercase().ends_with(".md") {
            return Ok(json!({
                "path": rel,
                "kind": "attachment",
                "size": st.len(),
                "mtime": mtime,
                "ctime": ctime,
            }));
        }

        let raw =
            read_utf8(&abs).map_err(|e| anyhow!(node_fs_error_message(&e, "open", &abs, None)))?;
        // Malformed frontmatter: count the whole file as body, as the parser does.
        let (fm, body) = match frontmatter::parse(&raw) {
            Ok(parsed) => (parsed.data, parsed.content),
            Err(_) => (Frontmatter::new(), raw.as_str()),
        };

        let (indexed, outgoing, incoming, unresolved) = ctx.db.with(|conn| {
            let outgoing = get_edges_by_source(conn, &rel)?;
            let unresolved = outgoing
                .iter()
                .filter(|e| e.target_id.starts_with("_stub/"))
                .count();
            anyhow::Ok((
                get_node(conn, &rel)?.is_some(),
                outgoing.len(),
                count_edges_by_target(conn, &rel)?,
                unresolved,
            ))
        })?;
        Ok(json!({
            "path": rel,
            "kind": "note",
            "size": st.len(),
            "mtime": mtime,
            "ctime": ctime,
            "indexed": indexed,
            "links": {
                "outgoing": outgoing,
                "incoming": incoming,
                "unresolved": unresolved,
            },
            "headings": extract_headings(&raw).len(),
            "tasks": count_tasks(&raw),
            "tags": collect_tags(&fm, body),
        }))
    })
}

/// The birth time, or the change time on file systems that do not record one.
fn birth_or_change_time(st: &Metadata) -> SystemTime {
    match st.created() {
        Ok(t) if t > UNIX_EPOCH => t,
        _ => {
            let secs = Duration::new(st.ctime().max(0) as u64, st.ctime_nsec().max(0) as u32);
            UNIX_EPOCH + secs
        }
    }
}

fn collect_tags(fm: &Frontmatter, body: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut tags = Vec::new();
    let mut add = |tag: String| {
        if seen.insert(tag.clone()) {
            tags.push(tag);
        }
    };
    for key in ["tags", "tag"] {
        let list: Vec<&str> = match fm.get(key) {
            Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
            Some(Value::String(s)) => s
                .split(|c: char| c == ',' || js::is_js_whitespace(c))
                .collect(),
            _ => Vec::new(),
        };
        for t in list {
            let t = js::trim(t);
            let tag = t.strip_prefix('#').unwrap_or(t);
            if !tag.is_empty() {
                add(tag.to_string());
            }
        }
    }
    for t in extract_inline_tags(body) {
        add(t);
    }
    tags
}
