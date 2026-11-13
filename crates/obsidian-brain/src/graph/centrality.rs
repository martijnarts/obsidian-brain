//! PageRank and betweenness centrality.
//!
//! Both follow graphology-metrics 2.4 (pagerank.js, betweenness.js and the
//! indexed Brandes routine of graphology-shortest-path), so scores match it
//! wherever its index is correct. graphology mis-indexes graphs with
//! self-loops and graphs of exactly 128 or 32768 edges, and wraps path
//! counts at 2^32; this port does not copy those bugs.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

use serde::Serialize;

use super::builder::UndirectedGraph;

/// The PageRank damping factor (graphology's `alpha`).
pub const PAGERANK_ALPHA: f64 = 0.85;
pub const PAGERANK_MAX_ITERATIONS: usize = 1000;
pub const PAGERANK_TOLERANCE: f64 = 1e-6;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RankedNode {
    pub id: String,
    pub title: String,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CentralityError {
    #[error("graphology-metrics/centrality/pagerank: failed to converge.")]
    PageRankDidNotConverge,
}

/// graphology-indices' `WeightedNeighborhoodIndex` over the outbound edges
/// of an undirected graph, every edge weighing 1: the neighbours of node
/// `i` are `neighborhood[starts[i]..starts[i + 1]]`. A self-loop is one
/// entry.
struct WeightedIndex {
    neighborhood: Vec<usize>,
    weights: Vec<f64>,
    out_degrees: Vec<f64>,
    starts: Vec<usize>,
}

impl WeightedIndex {
    fn new(g: &UndirectedGraph) -> Self {
        let order = g.order();
        let mut neighborhood = Vec::with_capacity(g.size() * 2);
        let mut out_degrees = Vec::with_capacity(order);
        let mut starts = Vec::with_capacity(order + 1);
        for i in 0..order {
            starts.push(neighborhood.len());
            neighborhood.extend_from_slice(g.adj_at(i));
            out_degrees.push(g.adj_at(i).len() as f64);
        }
        starts.push(neighborhood.len());
        Self {
            weights: vec![1.0; neighborhood.len()],
            neighborhood,
            out_degrees,
            starts,
        }
    }

    fn range(&self, i: usize) -> std::ops::Range<usize> {
        self.starts[i]..self.starts[i + 1]
    }
}

/// graphology-metrics `pagerank` on an undirected graph: power iteration
/// with dangling-node redistribution. Scores come back in node order.
pub fn pagerank_raw(
    g: &UndirectedGraph,
    alpha: f64,
    max_iterations: usize,
    tolerance: f64,
) -> Result<Vec<f64>, CentralityError> {
    let n = g.order();
    let p = 1.0 / n as f64;
    let index = WeightedIndex::new(g);

    let mut x = vec![p; n];
    let mut normalized = vec![0.0f64; index.weights.len()];
    let mut dangling = Vec::new();
    for i in 0..n {
        let d = index.out_degrees[i];
        if d == 0.0 {
            dangling.push(i);
        }
        for j in index.range(i) {
            normalized[j] = index.weights[j] / d;
        }
    }

    let mut iteration = 0;
    while iteration < max_iterations {
        let x_last = std::mem::replace(&mut x, vec![0.0; n]);

        let mut dangle_sum = 0.0;
        for &i in &dangling {
            dangle_sum += x_last[i];
        }
        dangle_sum *= alpha;

        for i in 0..n {
            for j in index.range(i) {
                let neighbor = index.neighborhood[j];
                x[neighbor] += alpha * x_last[i] * normalized[j];
            }
            x[i] += dangle_sum * p + (1.0 - alpha) * p;
        }

        let mut error = 0.0;
        for i in 0..n {
            error += (x[i] - x_last[i]).abs();
        }
        if error < n as f64 * tolerance {
            return Ok(x);
        }
        iteration += 1;
    }
    Err(CentralityError::PageRankDidNotConverge)
}

#[derive(Debug, Clone, Copy)]
struct HeapItem {
    cost: f64,
    count: u64,
    pred: usize,
    node: usize,
}

impl PartialEq for HeapItem {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for HeapItem {}

impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapItem {
    // `count` is unique, so cost then count is a total order.
    fn cmp(&self, other: &Self) -> Ordering {
        self.cost
            .total_cmp(&other.cost)
            .then(self.count.cmp(&other.count))
            .then(self.pred.cmp(&other.pred))
            .then(self.node.cmp(&other.node))
    }
}

/// graphology-metrics `betweennessCentrality` with its defaults: the
/// Dijkstra variant of Brandes (edge weight 1), normalized by
/// `1 / ((n - 1)(n - 2))` when `n > 2`. Scores come back in node order.
/// Path counts are f64, so they do not wrap as graphology's `Uint32Array`
/// does.
pub fn betweenness_raw(g: &UndirectedGraph) -> Vec<f64> {
    let n = g.order();
    let index = WeightedIndex::new(g);

    let mut stack = vec![0usize; n];
    let mut stack_size = 0usize;
    let mut sigma = vec![0.0f64; n];
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut dist = vec![-1.0f64; n];
    let mut seen = vec![-1.0f64; n];
    let mut delta = vec![0.0f64; n];
    let mut centralities = vec![0.0f64; n];
    let mut heap = BinaryHeap::new();

    for source in 0..n {
        let mut count: u64 = 0;
        for v in 0..n {
            preds[v].clear();
            sigma[v] = 0.0;
            dist[v] = -1.0;
            seen[v] = -1.0;
        }
        sigma[source] = 1.0;
        seen[source] = 0.0;
        heap.push(Reverse(HeapItem {
            cost: 0.0,
            count,
            pred: source,
            node: source,
        }));
        count += 1;

        while let Some(Reverse(item)) = heap.pop() {
            let v = item.node;
            if dist[v] != -1.0 {
                continue;
            }
            stack[stack_size] = v;
            stack_size += 1;
            dist[v] = item.cost;
            sigma[v] += sigma[item.pred];

            for j in index.range(v) {
                let w = index.neighborhood[j];
                let cost = item.cost + index.weights[j];
                if dist[w] == -1.0 && (seen[w] == -1.0 || cost < seen[w]) {
                    seen[w] = cost;
                    heap.push(Reverse(HeapItem {
                        cost,
                        count,
                        pred: v,
                        node: w,
                    }));
                    count += 1;
                    sigma[w] = 0.0;
                    preds[w].clear();
                    preds[w].push(v);
                } else if cost == seen[w] {
                    sigma[w] += sigma[v];
                    preds[w].push(v);
                }
            }
        }

        for &v in &stack[..stack_size] {
            delta[v] = 0.0;
        }

        while stack_size != 0 {
            stack_size -= 1;
            let w = stack[stack_size];
            let coefficient = (1.0 + delta[w]) / sigma[w];
            for &v in &preds[w] {
                delta[v] += sigma[v] * coefficient;
            }
            if w != source {
                centralities[w] += delta[w];
            }
        }
    }

    if n > 2 {
        let scale = 1.0 / ((n - 1) * (n - 2)) as f64;
        for c in &mut centralities {
            *c *= scale;
        }
    }
    centralities
}

/// PageRank on the subgraph of nodes with at least one edge. Isolated
/// nodes score 0; graphology's PageRank does not converge with them in.
///
/// Returns `(id, score)` pairs: the isolated nodes first, then the
/// connected ones, each in node order. Callers that sort the result keep
/// this order among ties.
pub fn page_rank(g: &UndirectedGraph) -> Result<Vec<(String, f64)>, CentralityError> {
    let mut scores = Vec::with_capacity(g.order());
    let mut connected = UndirectedGraph::new();
    for (i, id) in g.node_ids().iter().enumerate() {
        if g.degree_at(i) > 0 {
            connected.add_node(id, g.title_at(i));
        } else {
            scores.push((id.clone(), 0.0));
        }
    }
    for &(s, t) in g.edges_raw() {
        // add_edge skips edges with a missing end and repeated pairs.
        connected.add_edge(g.id_at(s), g.id_at(t));
    }
    if connected.order() == 0 {
        return Ok(scores);
    }
    let pr = pagerank_raw(
        &connected,
        PAGERANK_ALPHA,
        PAGERANK_MAX_ITERATIONS,
        PAGERANK_TOLERANCE,
    )?;
    scores.extend(connected.node_ids().iter().cloned().zip(pr));
    Ok(scores)
}

fn sort_desc(entries: &mut [(usize, f64)]) {
    entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
}

/// The top `limit` nodes by raw betweenness (as graphology normalizes it).
pub fn betweenness_centrality_top(g: &UndirectedGraph, limit: usize) -> Vec<RankedNode> {
    let mut entries: Vec<(usize, f64)> = betweenness_raw(g).into_iter().enumerate().collect();
    sort_desc(&mut entries);
    entries
        .into_iter()
        .take(limit)
        .map(|(i, score)| RankedNode {
            id: g.id_at(i).to_string(),
            title: g.title_at(i).to_string(),
            score,
        })
        .collect()
}

/// The top `limit` nodes by betweenness, divided by `n * (n - 1) / 2` so
/// scores compare across vaults of any size, and clamped into [0, 1].
/// Graphs with fewer than two nodes give an empty list.
pub fn betweenness_centrality_normalized(g: &UndirectedGraph, limit: usize) -> Vec<RankedNode> {
    let n = g.order();
    if n < 2 {
        return Vec::new();
    }
    let max_paths = (n * (n - 1)) as f64 / 2.0;
    let mut entries: Vec<(usize, f64)> = betweenness_raw(g)
        .into_iter()
        .enumerate()
        .map(|(i, s)| (i, s / max_paths))
        .collect();
    sort_desc(&mut entries);
    entries
        .into_iter()
        .take(limit)
        .map(|(i, score)| RankedNode {
            id: g.id_at(i).to_string(),
            title: g.title_at(i).to_string(),
            score: if score.is_nan() {
                score
            } else {
                score.clamp(0.0, 1.0)
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::builder::KnowledgeGraph;
    use crate::store::db::open_db;
    use crate::store::edges::insert_edge;
    use crate::store::nodes::upsert_node;
    use crate::types::{ParsedEdge, ParsedNode};
    use rusqlite::Connection;
    use serde_json::{Value, json};

    fn node(db: &Connection, id: &str, title: &str, stub: bool) {
        let fm = if stub {
            json!({"_stub": true})
        } else {
            json!({})
        };
        upsert_node(
            db,
            &ParsedNode {
                id: id.into(),
                title: title.into(),
                content: String::new(),
                frontmatter: fm.as_object().unwrap().clone(),
            },
        )
        .unwrap();
    }

    fn edge(db: &Connection, s: &str, t: &str, c: &str) {
        insert_edge(
            db,
            &ParsedEdge {
                source_id: s.into(),
                target_id: t.into(),
                context: c.into(),
                target_subpath: None,
            },
        )
        .unwrap();
    }

    fn kg(nodes: &[&str], edges: &[(&str, &str)]) -> KnowledgeGraph {
        let db = open_db(":memory:").unwrap();
        for id in nodes {
            node(&db, id, id, false);
        }
        for (s, t) in edges {
            edge(&db, s, t, &format!("{s}->{t}"));
        }
        KnowledgeGraph::from_store(&db, false).unwrap()
    }

    fn small() -> KnowledgeGraph {
        kg(
            &["a.md", "b.md", "c.md", "d.md"],
            &[("a.md", "b.md"), ("b.md", "c.md"), ("a.md", "c.md")],
        )
    }

    fn score(scores: &[(String, f64)], id: &str) -> f64 {
        scores.iter().find(|(i, _)| i == id).unwrap().1
    }

    #[test]
    fn page_rank_scores_every_node_and_zeroes_isolates() {
        let scores = page_rank(&small().to_undirected()).unwrap();
        assert_eq!(scores.len(), 4);
        for id in ["a.md", "b.md", "c.md"] {
            assert!(score(&scores, id) > 0.0);
        }
        assert_eq!(score(&scores, "d.md"), 0.0);
        assert!(scores.iter().all(|(_, v)| v.is_finite()));
    }

    #[test]
    fn page_rank_of_all_isolated_nodes_is_zero() {
        let scores = page_rank(&kg(&["x.md", "y.md"], &[]).to_undirected()).unwrap();
        assert_eq!(scores, [("x.md".into(), 0.0), ("y.md".into(), 0.0)]);
    }

    #[test]
    fn betweenness_top_returns_and_limits_results() {
        let u = small().to_undirected();
        assert!(!betweenness_centrality_top(&u, 10).is_empty());
        assert!(betweenness_centrality_top(&u, 2).len() <= 2);
    }

    #[test]
    fn star_hub_outscores_every_spoke() {
        let spokes = ["s1.md", "s2.md", "s3.md", "s4.md", "s5.md"];
        let mut nodes = vec!["hub.md"];
        nodes.extend(spokes);
        let edges: Vec<_> = spokes.iter().map(|s| (*s, "hub.md")).collect();
        let u = kg(&nodes, &edges).to_undirected();
        let top = betweenness_centrality_top(&u, 10);
        assert_eq!(top[0].id, "hub.md");
        assert!(top[1..].iter().all(|r| top[0].score > r.score));
        let pr = page_rank(&u).unwrap();
        for s in spokes {
            assert!(score(&pr, "hub.md") > score(&pr, s));
        }
    }

    #[test]
    fn chain_middle_outscores_endpoints() {
        let ids = ["A.md", "B.md", "C.md", "D.md", "E.md"];
        let edges: Vec<_> = ids.windows(2).map(|w| (w[0], w[1])).collect();
        let top = betweenness_centrality_top(&kg(&ids, &edges).to_undirected(), 10);
        let s = |id: &str| top.iter().find(|r| r.id == id).unwrap().score;
        assert!(s("C.md") > s("A.md") && s("C.md") > s("E.md"));
        assert!(s("B.md") > s("A.md") && s("D.md") > s("E.md"));
    }

    #[test]
    fn normalized_betweenness_is_empty_below_two_nodes() {
        assert!(
            betweenness_centrality_normalized(&kg(&["a.md"], &[]).to_undirected(), 10).is_empty()
        );
    }

    #[test]
    fn page_rank_reports_non_convergence() {
        let u = small().to_undirected();
        assert_eq!(
            pagerank_raw(&u, PAGERANK_ALPHA, 1, PAGERANK_TOLERANCE)
                .unwrap_err()
                .to_string(),
            "graphology-metrics/centrality/pagerank: failed to converge."
        );
    }

    fn parse(v: &Value) -> f64 {
        v.as_str().unwrap().parse().unwrap()
    }

    fn graph_of(case: &Value) -> UndirectedGraph {
        let db = open_db(":memory:").unwrap();
        for n in case["nodes"].as_array().unwrap() {
            let (id, title) = (n[0].as_str().unwrap(), n[1].as_str().unwrap());
            node(&db, id, title, n[2].as_bool().unwrap());
        }
        for e in case["edges"].as_array().unwrap() {
            let (s, t) = (e[0].as_str().unwrap(), e[1].as_str().unwrap());
            edge(&db, s, t, e[2].as_str().unwrap());
        }
        let include_stubs = case["includeStubs"].as_bool().unwrap();
        KnowledgeGraph::from_store(&db, include_stubs)
            .unwrap()
            .to_undirected()
    }

    /// Scores recorded from graphology-metrics through the TypeScript code
    /// (testdata/generate.mts), for graphs where its index is correct: no
    /// self-loops and not 128 or 32768 edges. Every value must match to
    /// the bit.
    #[test]
    fn matches_graphology_exactly() {
        let cases: Vec<Value> =
            serde_json::from_str(include_str!("testdata/graphology.json")).unwrap();
        assert_eq!(cases.len(), 2);
        for case in &cases {
            let name = case["name"].as_str().unwrap();
            let u = graph_of(case);

            let pr = page_rank(&u).unwrap();
            let want: Vec<(String, f64)> = case["pageRank"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| (p[0].as_str().unwrap().to_string(), parse(&p[1])))
                .collect();
            assert_eq!(pr, want, "{name}: pageRank");

            for (key, got) in [
                ("betweennessTop", betweenness_centrality_top(&u, 1000)),
                (
                    "betweennessNormalized",
                    betweenness_centrality_normalized(&u, 1000),
                ),
            ] {
                let want: Vec<RankedNode> = case[key]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| RankedNode {
                        id: r["id"].as_str().unwrap().into(),
                        title: r["title"].as_str().unwrap().into(),
                        score: parse(&r["score"]),
                    })
                    .collect();
                assert_eq!(got, want, "{name}: {key}");
            }
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-12 + 1e-9 * b.abs()
    }

    /// Scores from networkx (testdata/networkx_reference.py), an
    /// independent implementation, on graphs that include self-loops,
    /// parallel and reciprocal edges, stubs, isolated nodes and 128 edges.
    /// networkx sums in another order, so values match to a tolerance.
    #[test]
    fn matches_networkx() {
        let cases: Vec<Value> =
            serde_json::from_str(include_str!("testdata/networkx.json")).unwrap();
        assert_eq!(cases.len(), 5);
        for case in &cases {
            let name = case["name"].as_str().unwrap();
            let u = graph_of(case);

            let pr = page_rank(&u).unwrap();
            let want = case["pageRank"].as_array().unwrap();
            assert_eq!(pr.len(), want.len(), "{name}");
            for ((id, got), w) in pr.iter().zip(want) {
                assert_eq!(id, w[0].as_str().unwrap(), "{name}: pageRank order");
                let w = w[1].as_f64().unwrap();
                assert!(close(*got, w), "{name}: pageRank {id}: {got} vs {w}");
            }

            let raw = betweenness_raw(&u);
            let want = case["betweenness"].as_array().unwrap();
            assert_eq!(raw.len(), want.len(), "{name}");
            for (i, w) in want.iter().enumerate() {
                assert_eq!(u.node_ids()[i], w[0].as_str().unwrap(), "{name}");
                let w = w[1].as_f64().unwrap();
                assert!(
                    close(raw[i], w),
                    "{name}: betweenness {i}: {} vs {w}",
                    raw[i]
                );
            }
        }
    }

    #[test]
    fn page_rank_sums_to_one_on_a_128_edge_path() {
        let ids: Vec<String> = (0..129).map(|i| format!("p{i:03}.md")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let edges: Vec<_> = refs.windows(2).map(|w| (w[0], w[1])).collect();
        let pr = page_rank(&kg(&refs, &edges).to_undirected()).unwrap();
        let sum: f64 = pr.iter().map(|(_, v)| v).sum();
        assert!((sum - 1.0).abs() < 1e-9, "{sum}");
        // The path is symmetric, so its two ends score the same.
        assert!(close(score(&pr, "p000.md"), score(&pr, "p128.md")));
    }

    #[test]
    fn a_self_loop_changes_no_betweenness() {
        let ids = ["a.md", "b.md", "c.md", "d.md", "e.md"];
        let edges = [
            ("a.md", "b.md"),
            ("b.md", "c.md"),
            ("c.md", "d.md"),
            ("b.md", "e.md"),
        ];
        let plain = betweenness_raw(&kg(&ids, &edges).to_undirected());
        // The last node in index order carries the loop.
        let mut looped = edges.to_vec();
        looped.push(("e.md", "e.md"));
        assert_eq!(betweenness_raw(&kg(&ids, &looped).to_undirected()), plain);
        let pr = page_rank(&kg(&ids, &looped).to_undirected()).unwrap();
        let sum: f64 = pr.iter().map(|(_, v)| v).sum();
        assert!((sum - 1.0).abs() < 1e-9, "{sum}");
    }
}
