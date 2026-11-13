//! `ensure_block_id`: give a block a stable `^id` so other notes can link
//! or embed it. Idempotent: a block that already carries an id keeps it.

use anyhow::{Result, bail};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};

use super::patch::create_patch;
use crate::mcp::tool::ToolSpec;
use crate::resolve::single_note::resolve_single_note;
use crate::store::nodes::all_node_ids;
use crate::tools::support::background_reindex::run_background_reindex;
use crate::vault::block_id::{
    BLOCK_ID, all_block_ids, attach_block_id, existing_block_id, find_heading_lines,
    generate_block_id, locate_block,
};
use crate::vault::vault_path::{read_note_file, write_file_atomic};

const DESCRIPTION: &str = "Return the `^block-id` of a block in a note, adding one when it has none, plus a ready `[[Note#^id]]` link. Target the block by `line` or by `heading` (the heading line itself). Placement follows Obsidian: ` ^id` at the end of a paragraph, list item or heading; `^id` on its own line after a table, quote, callout or code block.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Path or fuzzy match of the note.
    name: String,
    /// 1-based line inside the block.
    #[schemars(range(min = 1))]
    line: Option<u64>,
    /// Heading text; the id goes on that heading line.
    heading: Option<String>,
    /// Id to write when the block has none: letters, digits, dashes. Default: random 6 characters.
    id: Option<String>,
    /// If true, return the id and diff without writing.
    dry_run: Option<bool>,
}

/// 0-based index of 1-based `line`, checked against the file length.
fn line_target(line: usize, line_count: usize, path: &str) -> Result<usize> {
    if line > line_count {
        bail!("Line {line} is past the end of \"{path}\" ({line_count} lines).");
    }
    Ok(line - 1)
}

fn heading_target(lines: &[&str], heading: &str, path: &str) -> Result<usize> {
    let found = find_heading_lines(lines, heading);
    match found.as_slice() {
        [] => bail!("No heading \"{heading}\" in \"{path}\"."),
        [only] => Ok(*only),
        many => bail!(
            "Heading \"{heading}\" appears {} times in \"{path}\" (lines {}). Pass `line` instead.",
            many.len(),
            many.iter()
                .map(|i| (i + 1).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `path.posix.basename(path, ".md")`.
fn basename_md(path: &str) -> &str {
    let base = path.rsplit('/').next().unwrap_or(path);
    match base.strip_suffix(".md") {
        Some(stem) if !stem.is_empty() => stem,
        _ => base,
    }
}

/// The note's link text: its basename, or its path without `.md` when
/// another note in the vault shares the basename.
fn link_target(path: &str, ids: &[String]) -> String {
    let base = basename_md(path);
    let clash = ids
        .iter()
        .any(|id| id != path && !id.starts_with("_stub/") && basename_md(id) == base);
    if clash {
        path.strip_suffix(".md").unwrap_or(path).to_string()
    } else {
        base.to_string()
    }
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("ensure_block_id", DESCRIPTION, |ctx, args| async move {
        if args.line.is_none() == args.heading.is_none() {
            bail!("Pass exactly one of `line` or `heading`.");
        }
        if let Some(id) = &args.id
            && !BLOCK_ID.is_match(id)
        {
            bail!("Block id \"{id}\" is invalid: use only letters, digits and dashes.");
        }

        let name = args.name.clone();
        let (id, node_ids) = ctx
            .db
            .run(move |c| -> Result<_> {
                Ok((resolve_single_note(&name, c, false)?, all_node_ids(c)?))
            })
            .await?;
        let note = read_note_file(&ctx.config.vault_path, &id)?;
        let lines: Vec<&str> = note.content.split('\n').collect();
        let target = match (args.line, &args.heading) {
            (Some(line), _) => line_target(line as usize, lines.len(), &note.rel)?,
            (None, Some(heading)) => heading_target(&lines, heading, &note.rel)?,
            (None, None) => unreachable!("checked above"),
        };

        let block = locate_block(&lines, target)?;
        let link = |id: &str| format!("[[{}#^{id}]]", link_target(&note.rel, &node_ids));

        let mut out = Map::new();
        out.insert("path".into(), note.rel.clone().into());
        if let Some(existing) = existing_block_id(&lines, block) {
            out.insert("id".into(), existing.id.clone().into());
            out.insert("created".into(), false.into());
            out.insert("line".into(), (existing.line + 1).into());
            out.insert("link".into(), link(&existing.id).into());
            if let Some(requested) = args.id.filter(|r| *r != existing.id) {
                out.insert("requestedIdIgnored".into(), requested.into());
            }
            return Ok(Value::Object(out));
        }

        let taken = all_block_ids(&lines);
        if let Some(requested) = &args.id
            && taken.contains(requested)
        {
            bail!("Block id ^{requested} is already used in \"{}\".", note.rel);
        }
        let id = args.id.clone().unwrap_or_else(|| generate_block_id(&taken));
        let written = attach_block_id(&lines, block, &id);
        let next = written.lines.join("\n");
        let diff = create_patch(&note.rel, &note.content, &next, "original", "proposed");

        let dry_run = args.dry_run == Some(true);
        if !dry_run {
            write_file_atomic(&note.abs, &next)?;
            run_background_reindex(&ctx);
        }
        out.insert("id".into(), id.clone().into());
        out.insert("created".into(), true.into());
        out.insert("dryRun".into(), dry_run.into());
        out.insert("line".into(), (written.line + 1).into());
        out.insert("link".into(), link(&id).into());
        out.insert("diff".into(), diff.into());
        Ok(Value::Object(out))
    })
}
