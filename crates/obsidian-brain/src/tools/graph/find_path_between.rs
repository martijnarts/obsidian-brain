//! `find_path_between`: every simple link path between two notes, and
//! optionally their shared neighbours.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::graph::KnowledgeGraph;
use crate::graph::pathfinding::{common_neighbors, find_paths};
use crate::mcp::tool::ToolSpec;
use crate::resolve::single_note::resolve_single_note;

const DESCRIPTION: &str = "Find link paths between two notes. Returns all simple paths up to maxDepth edges, optionally including their shared neighbors. Broken-wikilink stub nodes are excluded by default — they are degree-1 dead ends in the undirected graph and will block legitimate paths if left in. Pass `includeStubs: true` to include them.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Source note (path or fuzzy match).
    from: String,
    /// Target note (path or fuzzy match).
    to: String,
    /// Maximum path length in hops. Default 3.
    #[serde(default, deserialize_with = "super::opt_positive")]
    #[schemars(range(min = 1))]
    max_depth: Option<u64>,
    /// Also return notes that both `from` and `to` link to (shared neighbors).
    include_common: Option<bool>,
    /// Default `false`. Set `true` to include broken-wikilink stub nodes (`frontmatter._stub: true`) in the path search.
    include_stubs: Option<bool>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("find_path_between", DESCRIPTION, |ctx, args| async move {
        ctx.db
            .run(move |c| -> anyhow::Result<_> {
                let from_id = resolve_single_note(&args.from, c, true)?;
                let to_id = resolve_single_note(&args.to, c, true)?;
                let g = KnowledgeGraph::from_store(c, args.include_stubs.unwrap_or(false))?;
                let paths = find_paths(&g, &from_id, &to_id, args.max_depth.unwrap_or(3) as usize);
                if args.include_common == Some(true) {
                    let common = common_neighbors(&g, &from_id, &to_id);
                    return Ok(json!({ "paths": paths, "common": common }));
                }
                Ok(json!({ "paths": paths }))
            })
            .await
    })
}
