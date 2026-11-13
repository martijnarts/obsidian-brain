//! `find_connections`: the notes within N hops of a note, as a flat list
//! or as the induced subgraph, wrapped with follow-up hints.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::graph::KnowledgeGraph;
use crate::graph::pathfinding::{extract_subgraph, find_neighbors};
use crate::mcp::tool::ToolSpec;
use crate::resolve::single_note::resolve_single_note;
use crate::tools::support::hints::{FindConnectionsHintNeighbor, compute_find_connections_hints};

const DESCRIPTION: &str = "Find notes linked to (from or to) a given note, up to N hops. Optionally return the full subgraph instead of a flat list. Response is wrapped as `{data, context}` where `context.next_actions` suggests follow-ups like tracing a path to the furthest neighbour via `find_path_between`. Broken-wikilink stub neighbours are excluded by default; pass `includeStubs: true` to include them.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Starting note (path or fuzzy match).
    name: String,
    /// Number of hops to traverse. Default 1, max 3.
    #[serde(default, deserialize_with = "super::opt_positive")]
    #[schemars(range(min = 1))]
    depth: Option<u64>,
    /// Return all edges in the neighborhood as a full subgraph instead of a flat list.
    return_subgraph: Option<bool>,
    /// Default `false`. Set `true` to include broken-wikilink stub neighbours (`frontmatter._stub: true`).
    include_stubs: Option<bool>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("find_connections", DESCRIPTION, |ctx, args| async move {
        ctx.db
            .run(move |c| -> anyhow::Result<_> {
                let id = resolve_single_note(&args.name, c, true)?;
                let g = KnowledgeGraph::from_store(c, args.include_stubs.unwrap_or(false))?;
                let d = args.depth.unwrap_or(1) as usize;
                if args.return_subgraph == Some(true) {
                    let sub = extract_subgraph(&g, &id, d, Some(c))?;
                    let neighbors: Vec<FindConnectionsHintNeighbor> = sub
                        .nodes
                        .iter()
                        .filter(|n| n.id != id)
                        .map(|n| FindConnectionsHintNeighbor {
                            id: n.id.clone(),
                            title: Some(n.title.clone()),
                        })
                        .collect();
                    let context = compute_find_connections_hints(&id, &neighbors);
                    return Ok(json!({ "data": sub, "context": context }));
                }
                let neighbors = find_neighbors(&g, &id, d);
                let hint_input: Vec<FindConnectionsHintNeighbor> = neighbors
                    .iter()
                    .map(|n| FindConnectionsHintNeighbor {
                        id: n.id.clone(),
                        title: Some(n.title.clone()),
                    })
                    .collect();
                let context = compute_find_connections_hints(&id, &hint_input);
                Ok(json!({ "data": neighbors, "context": context }))
            })
            .await
    })
}
