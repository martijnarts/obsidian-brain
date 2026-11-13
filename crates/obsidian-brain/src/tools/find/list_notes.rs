//! `list_notes`: the indexed notes, filtered by folder or tag.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};

use super::utf16_cmp;
use crate::mcp::tool::ToolSpec;
use crate::store::nodes::{all_node_ids, get_node, is_stub};
use crate::store::sync::all_sync_mtimes;
use crate::util::js::locale_compare;
use crate::vault::tags::{note_tags, tag_matches};

const DESCRIPTION: &str = "List notes in the vault. Optionally filter by directory prefix or by tag (frontmatter or inline; `a` also matches `a/b`). Pass `sortBy: \"mtime\"` for the most recently modified notes first. Pass `includeStubs: false` to exclude unresolved wiki-link targets (nodes with `frontmatter._stub: true`) and see only real on-disk notes.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum SortBy {
    Path,
    Mtime,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Restrict to notes under this subdirectory prefix.
    directory: Option<String>,
    /// Restrict to notes with this tag or a tag nested below it.
    tag: Option<String>,
    /// Default `path`. `mtime` sorts newest first and adds `mtime` to each result.
    sort_by: Option<SortBy>,
    /// Max results to return. Default 100.
    #[schemars(range(min = 1))]
    limit: Option<u64>,
    /// Default `true`. Set `false` to exclude unresolved wiki-link targets.
    include_stubs: Option<bool>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("list_notes", DESCRIPTION, |ctx, args| async move {
        ctx.db.run(move |c| list_notes(c, args)).await
    })
}

fn list_notes(c: &rusqlite::Connection, args: Args) -> anyhow::Result<Value> {
    let mut ids = all_node_ids(c)?;
    ids.sort_by(|a, b| utf16_cmp(a, b));
    let cap = args.limit.unwrap_or(100) as usize;
    let exclude_stubs = args.include_stubs == Some(false);
    let by_mtime = args.sort_by == Some(SortBy::Mtime);

    let mut results: Vec<Map<String, Value>> = Vec::new();
    for id in ids {
        if let Some(dir) = &args.directory
            && !(id.starts_with(&format!("{dir}/")) || &id == dir)
        {
            continue;
        }
        let Some(node) = get_node(c, &id)? else {
            continue;
        };
        if exclude_stubs && is_stub(&node.frontmatter) {
            continue;
        }
        let tags = match node.frontmatter.get("tags") {
            Some(Value::Array(items)) => Value::Array(items.clone()),
            _ => Value::Array(Vec::new()),
        };
        if let Some(tag) = &args.tag
            && !note_tags(&node.frontmatter)
                .iter()
                .any(|t| tag_matches(t, tag))
        {
            continue;
        }
        let mut row = Map::new();
        row.insert("id".into(), node.id.into());
        row.insert("title".into(), node.title.into());
        row.insert("tags".into(), tags);
        row.insert("frontmatter".into(), Value::Object(node.frontmatter));
        results.push(row);
        if !by_mtime && results.len() >= cap {
            break;
        }
    }

    if !by_mtime {
        return Ok(Value::Array(
            results.into_iter().map(Value::Object).collect(),
        ));
    }

    // Notes the indexer has not recorded yet (stubs, fresh writes) sort last.
    let mtimes = all_sync_mtimes(c)?;
    let mut keyed: Vec<(Map<String, Value>, f64)> = results
        .into_iter()
        .map(|r| {
            let ms = r["id"]
                .as_str()
                .and_then(|id| mtimes.get(id))
                .copied()
                .unwrap_or(0.0);
            (r, ms)
        })
        .collect();
    keyed.sort_by(|(a, ma), (b, mb)| {
        mb.partial_cmp(ma)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                locale_compare(
                    a["id"].as_str().unwrap_or(""),
                    b["id"].as_str().unwrap_or(""),
                )
            })
    });
    Ok(Value::Array(
        keyed
            .into_iter()
            .take(cap)
            .map(|(mut r, ms)| {
                if ms > 0.0
                    && let Some(iso) = iso_from_ms(ms)
                {
                    r.insert("mtime".into(), iso.into());
                }
                Value::Object(r)
            })
            .collect(),
    ))
}

/// `new Date(ms).toISOString()`; the fraction of a millisecond is dropped.
pub(super) fn iso_from_ms(ms: f64) -> Option<String> {
    let dt = chrono::DateTime::from_timestamp_millis(ms.trunc() as i64)?;
    Some(dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}
