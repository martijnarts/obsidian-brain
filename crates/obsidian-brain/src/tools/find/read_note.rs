//! `read_note`: a note's metadata and links, or its body too.

use std::collections::HashSet;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::check_range;
use crate::mcp::tool::ToolSpec;
use crate::resolve::single_note::resolve_single_note;
use crate::store::edges::{
    get_edge_summaries_by_source, get_edge_summaries_by_target, get_edges_by_source,
    get_edges_by_target,
};
use crate::store::nodes::get_node;
use crate::tools::support::{ReadNoteHintInput, compute_read_note_hints};
use crate::util::js;

const DESCRIPTION: &str = "Read a note's content. Brief mode (default) returns title + metadata + linked-note titles; full mode returns full content + edge context. Full mode also reports `truncated: true` when the body exceeded `maxContentLength` (default 2000 chars) and was sliced. Response is wrapped as `{data, context}` where `context.next_actions` suggests follow-ups like creating missing linked notes or exploring outgoing connections.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Brief,
    Full,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Path, filename, or fuzzy match for the note to read.
    name: String,
    /// Default `"brief"` (metadata + linked-note titles). `"full"` adds the body + edge context.
    mode: Option<Mode>,
    /// In `full` mode, max body chars before truncation. Default 2000.
    #[schemars(range(min = 1))]
    max_content_length: Option<u64>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("read_note", DESCRIPTION, |ctx, args| async move {
        check_range("maxContentLength", args.max_content_length, 1, None)?;
        ctx.db.run(move |c| read_note(c, args)).await
    })
}

fn read_note(c: &rusqlite::Connection, args: Args) -> anyhow::Result<Value> {
    let name = args.name;
    let id = resolve_single_note(&name, c, true)?;
    let Some(node) = get_node(c, &id)? else {
        anyhow::bail!("No note found matching \"{name}\"");
    };
    let exists = |id: &str| -> anyhow::Result<bool> { Ok(get_node(c, id)?.is_some()) };

    if args.mode.unwrap_or(Mode::Brief) == Mode::Brief {
        let outgoing_summaries = get_edge_summaries_by_source(c, &id)?;
        let incoming_summaries = get_edge_summaries_by_target(c, &id)?;
        let outgoing: Vec<Value> = outgoing_summaries
            .iter()
            .map(|e| json!({ "targetId": e.node_id, "targetTitle": e.title }))
            .collect();
        let incoming: Vec<Value> = incoming_summaries
            .iter()
            .map(|e| json!({ "sourceId": e.node_id, "sourceTitle": e.title }))
            .collect();
        let mut unresolved_links = Vec::new();
        for e in &outgoing_summaries {
            if !exists(&e.node_id)? {
                unresolved_links.push(e.node_id.clone());
            }
        }
        let context = compute_read_note_hints(&ReadNoteHintInput {
            id: node.id.clone(),
            outgoing: outgoing_summaries
                .iter()
                .map(|e| e.node_id.clone())
                .collect(),
            unresolved_links,
        });
        let data = json!({
            "id": node.id,
            "title": node.title,
            "frontmatter": node.frontmatter,
            "outgoing": outgoing,
            "incoming": incoming,
        });
        return Ok(json!({ "data": data, "context": context }));
    }

    let max = args.max_content_length.unwrap_or(2000) as usize;
    let truncated = js::utf16_len(&node.content) > max;
    let content = if truncated {
        js::utf16_slice(&node.content, 0, max)
    } else {
        node.content.clone()
    };
    let title_of = |id: &str| -> anyhow::Result<String> {
        Ok(get_node(c, id)?.map_or_else(|| id.to_string(), |n| n.title))
    };

    let outgoing_edges = get_edges_by_source(c, &id)?;
    let incoming_edges = get_edges_by_target(c, &id)?;
    let mut outgoing = Vec::new();
    let mut outgoing_ids = Vec::new();
    let mut unresolved_links = Vec::new();
    for e in &outgoing_edges {
        outgoing.push(json!({
            "targetId": e.target_id,
            "targetTitle": title_of(&e.target_id)?,
            "context": e.context,
        }));
        outgoing_ids.push(e.target_id.clone());
        if !exists(&e.target_id)? {
            unresolved_links.push(e.target_id.clone());
        }
    }
    let mut incoming = Vec::new();
    for e in &incoming_edges {
        incoming.push(json!({
            "sourceId": e.source_id,
            "sourceTitle": title_of(&e.source_id)?,
            "context": e.context,
        }));
    }
    let context = compute_read_note_hints(&ReadNoteHintInput {
        id: node.id.clone(),
        outgoing: dedupe(outgoing_ids),
        unresolved_links: dedupe(unresolved_links),
    });
    let data = json!({
        "id": node.id,
        "title": node.title,
        "frontmatter": node.frontmatter,
        "content": content,
        "truncated": truncated,
        "outgoing": outgoing,
        "incoming": incoming,
    });
    Ok(json!({ "data": data, "context": context }))
}

/// `Array.from(new Set(ids))`: first occurrence wins.
fn dedupe(ids: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    ids.into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect()
}
