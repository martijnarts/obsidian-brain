//! `rank_notes`: PageRank ("influence") and normalized betweenness
//! ("bridging") over the undirected link graph.

use std::cmp::Ordering;
use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::graph::KnowledgeGraph;
use crate::graph::centrality::{betweenness_centrality_normalized, page_rank};
use crate::mcp::tool::ToolSpec;

const DESCRIPTION: &str = "Rank notes by importance: 'influence' (densely-connected hubs), 'bridging' (notes that connect otherwise-separate topic clusters), or both. Credibility guards (I): by default, `influence` excludes notes with fewer than `minIncomingLinks: 2` incoming edges — this filters out random-orphan noise that makes PageRank feel meaningless on personal vaults. Pass `minIncomingLinks: 0` to see the unfiltered ranking. Bridging scores are normalized by graph size (divided by n*(n-1)/2) so values compare across vaults of different sizes — a bridging score of 0.5 means the same thing in any vault. Broken-wikilink stub targets are excluded by default; pass `includeStubs: true` to include them.";

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Metric {
    Influence,
    Bridging,
    Both,
}

fn two() -> u64 {
    2
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    #[schemars(
        description = "Ranking metric. Default `\"both\"`. `\"influence\"` = PageRank; `\"bridging\"` = betweenness centrality."
    )]
    metric: Option<Metric>,
    /// Max results to return. Default 20.
    #[schemars(range(min = 1))]
    limit: Option<u64>,
    /// Default `false`. Set `true` to include unresolved wiki-link target stubs (`frontmatter._stub: true`) in the ranked set. With stubs in, popular link targets dominate eigenvector-style centrality even when they have no real content behind them.
    #[serde(default)]
    include_stubs: bool,
    /// Minimum incoming links for influence ranking. Default 2. Pass 0 to see unfiltered PageRank.
    #[serde(default = "two")]
    min_incoming_links: u64,
}

fn entry(id: &str, title: &str, score: f64) -> Value {
    json!({ "id": id, "title": title, "score": score })
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("rank_notes", DESCRIPTION, |ctx, args| async move {
        ctx.db
            .run(move |c| -> anyhow::Result<Value> {
                let kg = KnowledgeGraph::from_store(c, args.include_stubs)?;
                // Incoming-link counts come from the directed graph: "incoming"
                // means nothing once the edges are undirected.
                let in_counts: HashMap<String, usize> = kg
                    .nodes()
                    .iter()
                    .map(|n| (n.id.clone(), kg.in_neighbors(&n.id).len()))
                    .collect();
                let g = kg.to_undirected();
                let lim = args.limit.unwrap_or(20) as usize;
                let min_in = args.min_incoming_links as usize;

                // A PageRank that does not converge fails the call, as in TS.
                let influence = || -> anyhow::Result<Value> {
                    let mut pr: Vec<(String, f64)> = page_rank(&g)?
                        .into_iter()
                        .filter(|(id, _)| in_counts.get(id).copied().unwrap_or(0) >= min_in)
                        .collect();
                    pr.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
                    Ok(Value::Array(
                        pr.into_iter()
                            .take(lim)
                            .map(|(id, score)| entry(&id, &g.node_title(&id), score))
                            .collect(),
                    ))
                };
                let bridging = || -> Value {
                    Value::Array(
                        betweenness_centrality_normalized(&g, lim)
                            .into_iter()
                            .map(|r| entry(&r.id, &r.title, r.score))
                            .collect(),
                    )
                };

                Ok(match args.metric.unwrap_or(Metric::Both) {
                    Metric::Influence => influence()?,
                    Metric::Bridging => bridging(),
                    Metric::Both => json!({ "influence": influence()?, "bridging": bridging() }),
                })
            })
            .await
    })
}
