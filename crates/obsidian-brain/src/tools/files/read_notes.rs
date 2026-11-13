//! `read_notes`: read several notes in one call. Each name resolves on its
//! own, so a missing or ambiguous name yields an `{name, error}` entry in
//! its slot and the rest of the batch still returns.

use anyhow::anyhow;
use rusqlite::Connection;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::mcp::tool::ToolSpec;
use crate::resolve::single_note::resolve_single_note;
use crate::store::nodes::get_node;
use crate::util::js;

const DESCRIPTION: &str = "Read up to 20 notes in one call. Returns one entry per name, in order: `{path, title, frontmatter, content, truncated}`, or `{name, error}` when that name does not resolve.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Paths, filenames, or fuzzy matches of the notes to read.
    #[schemars(length(min = 1, max = 20))]
    names: Vec<String>,
    /// Max body chars per note before truncation. Default 2000.
    #[schemars(range(min = 1))]
    max_content_length: Option<u64>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("read_notes", DESCRIPTION, |ctx, args| async move {
        let max = args.max_content_length.unwrap_or(2000) as usize;
        let notes: Vec<Value> = ctx.db.with(|conn| {
            args.names
                .iter()
                .map(|name| {
                    read_one(conn, name, max)
                        .unwrap_or_else(|e| json!({ "name": name, "error": e.to_string() }))
                })
                .collect()
        });
        Ok(json!({ "notes": notes }))
    })
}

fn read_one(conn: &Connection, name: &str, max: usize) -> anyhow::Result<Value> {
    let path = resolve_single_note(name, conn, true)?;
    let node =
        get_node(conn, &path)?.ok_or_else(|| anyhow!("No note found matching \"{name}\""))?;
    let truncated = js::utf16_len(&node.content) > max;
    let content = if truncated {
        js::utf16_slice(&node.content, 0, max)
    } else {
        node.content
    };
    Ok(json!({
        "path": path,
        "title": node.title,
        "frontmatter": node.frontmatter,
        "content": content,
        "truncated": truncated,
    }))
}
