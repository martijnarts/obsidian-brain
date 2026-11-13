//! `read_note_part`: read one part of a note from disk, so a client can
//! look at a long note's outline and then fetch only the section it needs.
//! Line numbers are 1-based and count the frontmatter.

use anyhow::bail;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::args;
use crate::mcp::tool::ToolSpec;
use crate::resolve::single_note::resolve_single_note;
use crate::util::errors::node_fs_error_message;
use crate::vault::note_parts::{extract_headings, find_block, find_heading_section, slice_lines};
use crate::vault::vault_path::{read_utf8, resolve_vault_path};

const DESCRIPTION: &str = "Read part of a note from disk: its heading `outline`, the section under a `heading`, a range of `lines`, or the `block` that carries a `^blockId`. Line numbers are 1-based and count the frontmatter.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Outline,
    Heading,
    Lines,
    Block,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Path, filename, or fuzzy match for the note.
    name: String,
    /// Which part to read.
    mode: Mode,
    /// For `heading`: heading text, or a nested path like `Parent > Child`.
    heading: Option<String>,
    /// For `lines`: first line, 1-based, inclusive.
    #[serde(default, deserialize_with = "args::opt_positive")]
    #[schemars(range(min = 1))]
    start_line: Option<u64>,
    /// For `lines`: last line, inclusive. Default and maximum: the end of the note.
    #[serde(default, deserialize_with = "args::opt_positive")]
    #[schemars(range(min = 1))]
    end_line: Option<u64>,
    /// For `block`: the block id, with or without the leading `^`.
    block_id: Option<String>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("read_note_part", DESCRIPTION, |ctx, args| async move {
        let path = ctx
            .db
            .with(|conn| resolve_single_note(&args.name, conn, false))?;
        let abs = resolve_vault_path(&ctx.config.vault_path, &path)?.abs;
        let raw = read_utf8(&abs)
            .map_err(|e| anyhow::anyhow!(node_fs_error_message(&e, "open", &abs, None)))?;

        Ok(match args.mode {
            Mode::Outline => json!({ "path": path, "headings": extract_headings(&raw) }),
            Mode::Heading => {
                let Some(heading) = &args.heading else {
                    bail!("mode \"heading\" needs `heading`");
                };
                let s = find_heading_section(&raw, heading)?;
                json!({
                    "path": path,
                    "heading": s.heading,
                    "startLine": s.part.start_line,
                    "endLine": s.part.end_line,
                    "content": s.part.content,
                })
            }
            Mode::Lines => {
                let Some(start) = args.start_line else {
                    bail!("mode \"lines\" needs `startLine`");
                };
                let s = slice_lines(&raw, start as usize, args.end_line.map(|e| e as usize))?;
                json!({
                    "path": path,
                    "startLine": s.part.start_line,
                    "endLine": s.part.end_line,
                    "totalLines": s.total_lines,
                    "content": s.part.content,
                })
            }
            Mode::Block => {
                let Some(block_id) = &args.block_id else {
                    bail!("mode \"block\" needs `blockId`");
                };
                let b = find_block(&raw, block_id)?;
                json!({
                    "path": path,
                    "blockId": b.block_id,
                    "startLine": b.part.start_line,
                    "endLine": b.part.end_line,
                    "content": b.part.content,
                })
            }
        })
    })
}
