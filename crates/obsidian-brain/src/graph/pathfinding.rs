//! Neighbourhoods, paths and subgraphs over the link graph.

use std::collections::{HashSet, VecDeque};

use rusqlite::Connection;
use serde::Serialize;

use super::builder::{KnowledgeGraph, UndirectedGraph};
use crate::store::Result;
use crate::store::nodes::get_node;
use crate::types::{PathEdge, PathResult, SubgraphNode, SubgraphResult};

/// A neighbour found by traversal.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NeighborInfo {
    pub id: String,
    pub title: String,
}

/// Out-neighbours then in-neighbours of node `i`, without repeats.
fn undirected_neighbors(graph: &KnowledgeGraph, i: usize) -> Vec<usize> {
    let mut seen = HashSet::new();
    graph
        .out_indices(i)
        .iter()
        .chain(graph.in_indices(i))
        .copied()
        .filter(|j| seen.insert(*j))
        .collect()
}

/// BFS from `node_id` up to `depth` hops, following links either way. The
/// seed is not returned; an unknown seed gives an empty list.
pub fn find_neighbors(graph: &KnowledgeGraph, node_id: &str, depth: usize) -> Vec<NeighborInfo> {
    let Some(seed) = graph.index_of(node_id) else {
        return Vec::new();
    };
    let nodes = graph.nodes();
    let mut visited = HashSet::from([seed]);
    let mut result = Vec::new();
    let mut queue = VecDeque::from([(seed, 0usize)]);
    while let Some((id, d)) = queue.pop_front() {
        if d >= depth {
            continue;
        }
        for nid in undirected_neighbors(graph, id) {
            if visited.insert(nid) {
                result.push(NeighborInfo {
                    id: nodes[nid].id.clone(),
                    title: nodes[nid].title.clone(),
                });
                queue.push_back((nid, d + 1));
            }
        }
    }
    result
}

/// Every simple path from `from` to `to` with at most `max_depth` edges,
/// walking links either way. Each hop carries the context of its edge.
pub fn find_paths(
    graph: &KnowledgeGraph,
    from: &str,
    to: &str,
    max_depth: usize,
) -> Vec<PathResult> {
    if !graph.has_node(from) || !graph.has_node(to) {
        return Vec::new();
    }
    let undirected = graph.to_undirected();
    find_all_simple_paths(&undirected, from, to, max_depth)
        .into_iter()
        .map(|nodes| {
            let edges = nodes
                .windows(2)
                .map(|pair| PathEdge {
                    source_id: pair[0].clone(),
                    target_id: pair[1].clone(),
                    context: first_edge_context(graph, &pair[0], &pair[1]),
                })
                .collect();
            let length = nodes.len() - 1;
            PathResult {
                nodes,
                edges,
                length,
            }
        })
        .collect()
}

/// The intersection of the undirected neighbourhoods of `a` and `b`.
pub fn common_neighbors(graph: &KnowledgeGraph, a: &str, b: &str) -> Vec<NeighborInfo> {
    let (Some(ia), Some(ib)) = (graph.index_of(a), graph.index_of(b)) else {
        return Vec::new();
    };
    let nb: HashSet<usize> = undirected_neighbors(graph, ib).into_iter().collect();
    let nodes = graph.nodes();
    undirected_neighbors(graph, ia)
        .into_iter()
        .filter(|i| nb.contains(i))
        .map(|i| NeighborInfo {
            id: nodes[i].id.clone(),
            title: nodes[i].title.clone(),
        })
        .collect()
}

/// BFS out from `seed` to `depth` hops and return the induced subgraph:
/// the visited nodes and every edge between them, in its stored
/// direction. With `db`, each node carries its frontmatter from the store;
/// without it, an empty map.
pub fn extract_subgraph(
    graph: &KnowledgeGraph,
    seed: &str,
    depth: usize,
    db: Option<&Connection>,
) -> Result<SubgraphResult> {
    let Some(start) = graph.index_of(seed) else {
        return Ok(SubgraphResult {
            nodes: Vec::new(),
            edges: Vec::new(),
        });
    };
    let mut order = vec![start];
    let mut visited = HashSet::from([start]);
    let mut queue = VecDeque::from([(start, 0usize)]);
    while let Some((id, d)) = queue.pop_front() {
        if d >= depth {
            continue;
        }
        for nid in undirected_neighbors(graph, id) {
            if visited.insert(nid) {
                order.push(nid);
                queue.push_back((nid, d + 1));
            }
        }
    }

    let all = graph.nodes();
    let mut nodes = Vec::with_capacity(order.len());
    for &i in &order {
        let frontmatter = match db {
            Some(conn) => get_node(conn, &all[i].id)?
                .map(|n| n.frontmatter)
                .unwrap_or_default(),
            None => Default::default(),
        };
        nodes.push(SubgraphNode {
            id: all[i].id.clone(),
            title: all[i].title.clone(),
            frontmatter,
        });
    }

    let mut edges = Vec::new();
    for &i in &order {
        for &j in graph.out_indices(i) {
            if visited.contains(&j) {
                edges.push(PathEdge {
                    source_id: all[i].id.clone(),
                    target_id: all[j].id.clone(),
                    context: first_edge_context(graph, &all[i].id, &all[j].id),
                });
            }
        }
    }
    Ok(SubgraphResult { nodes, edges })
}

/// The context of an edge between `src` and `tgt`, forward first, then
/// backward; `''` when there is none. graphology lists parallel edges
/// newest first, so the most recently added edge wins.
fn first_edge_context(graph: &KnowledgeGraph, src: &str, tgt: &str) -> String {
    graph
        .edge_contexts(src, tgt)
        .or_else(|| graph.edge_contexts(tgt, src))
        .and_then(|c| c.last())
        .cloned()
        .unwrap_or_default()
}

/// Depth-limited simple-path enumeration by DFS.
fn find_all_simple_paths(
    graph: &UndirectedGraph,
    from: &str,
    to: &str,
    max_depth: usize,
) -> Vec<Vec<String>> {
    struct Search<'a> {
        graph: &'a UndirectedGraph,
        target: usize,
        max_depth: usize,
        path: Vec<usize>,
        visited: HashSet<usize>,
        results: Vec<Vec<String>>,
    }

    impl Search<'_> {
        // The path holds one more node than it has edges.
        fn dfs(&mut self, current: usize) {
            if current == self.target {
                let path = self.path.iter().map(|&i| self.graph.id_at(i).to_string());
                self.results.push(path.collect());
                return;
            }
            if self.path.len() > self.max_depth {
                return;
            }
            for &neighbor in self.graph.adj_at(current) {
                if self.visited.insert(neighbor) {
                    self.path.push(neighbor);
                    self.dfs(neighbor);
                    self.path.pop();
                    self.visited.remove(&neighbor);
                }
            }
        }
    }

    let (Some(from), Some(to)) = (graph.index_of(from), graph.index_of(to)) else {
        return Vec::new();
    };
    let mut search = Search {
        graph,
        target: to,
        max_depth,
        path: vec![from],
        visited: HashSet::from([from]),
        results: Vec::new(),
    };
    search.dfs(from);
    search.results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::db::open_db;
    use crate::store::edges::insert_edge;
    use crate::store::nodes::upsert_node;
    use crate::types::{ParsedEdge, ParsedNode};
    use serde_json::json;

    fn setup() -> (Connection, KnowledgeGraph) {
        let db = open_db(":memory:").unwrap();
        for (id, title) in [("a.md", "A"), ("b.md", "B"), ("c.md", "C"), ("d.md", "D")] {
            upsert_node(
                &db,
                &ParsedNode {
                    id: id.into(),
                    title: title.into(),
                    content: String::new(),
                    frontmatter: json!({"t": title}).as_object().unwrap().clone(),
                },
            )
            .unwrap();
        }
        for (s, t, c) in [
            ("a.md", "b.md", "A links to B"),
            ("b.md", "c.md", "B links to C"),
            ("a.md", "c.md", "A links to C"),
        ] {
            insert_edge(
                &db,
                &ParsedEdge {
                    source_id: s.into(),
                    target_id: t.into(),
                    context: c.into(),
                    target_subpath: None,
                },
            )
            .unwrap();
        }
        let kg = KnowledgeGraph::from_store(&db, false).unwrap();
        (db, kg)
    }

    fn ids(v: &[NeighborInfo]) -> Vec<&str> {
        v.iter().map(|n| n.id.as_str()).collect()
    }

    #[test]
    fn finds_neighbors_at_depth_one_and_two() {
        let (_db, kg) = setup();
        assert_eq!(ids(&find_neighbors(&kg, "a.md", 1)), ["b.md", "c.md"]);
        assert_eq!(ids(&find_neighbors(&kg, "a.md", 2)), ["b.md", "c.md"]);
        assert_eq!(ids(&find_neighbors(&kg, "c.md", 1)), ["a.md", "b.md"]);
    }

    #[test]
    fn unknown_seed_has_no_neighbors() {
        let (_db, kg) = setup();
        assert!(find_neighbors(&kg, "missing.md", 1).is_empty());
    }

    #[test]
    fn finds_paths_between_connected_nodes() {
        let (_db, kg) = setup();
        let paths = find_paths(&kg, "a.md", "c.md", 3);
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().any(|p| p.length == 1));
        assert!(paths.iter().any(|p| p.nodes.contains(&"b.md".to_string())));
    }

    #[test]
    fn no_paths_for_disconnected_or_unknown_nodes() {
        let (_db, kg) = setup();
        assert!(find_paths(&kg, "a.md", "d.md", 3).is_empty());
        assert!(find_paths(&kg, "a.md", "ghost.md", 3).is_empty());
    }

    #[test]
    fn finds_common_neighbors() {
        let (_db, kg) = setup();
        assert_eq!(ids(&common_neighbors(&kg, "a.md", "b.md")), ["c.md"]);
    }

    #[test]
    fn extracts_a_subgraph() {
        let (db, kg) = setup();
        let sub = extract_subgraph(&kg, "a.md", 1, None).unwrap();
        let ids: Vec<_> = sub.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["a.md", "b.md", "c.md"]);
        assert_eq!(sub.edges.len(), 3);
        assert!(sub.nodes[0].frontmatter.is_empty());
        let with_db = extract_subgraph(&kg, "a.md", 1, Some(&db)).unwrap();
        assert_eq!(with_db.nodes[0].frontmatter["t"], "A");
    }

    #[test]
    fn subgraph_of_unknown_seed_is_empty() {
        let (_db, kg) = setup();
        let sub = extract_subgraph(&kg, "ghost.md", 1, None).unwrap();
        assert!(sub.nodes.is_empty() && sub.edges.is_empty());
    }

    #[test]
    fn backward_traversal_keeps_edge_context() {
        let (_db, kg) = setup();
        let paths = find_paths(&kg, "b.md", "a.md", 2);
        let direct = paths
            .iter()
            .find(|p| p.length == 1 && p.nodes[1] == "a.md")
            .unwrap();
        assert_eq!(direct.edges[0].context, "A links to B");
    }

    #[test]
    fn subgraph_edges_carry_context() {
        let (_db, kg) = setup();
        let sub = extract_subgraph(&kg, "a.md", 2, None).unwrap();
        let ab = sub
            .edges
            .iter()
            .find(|e| e.source_id == "a.md" && e.target_id == "b.md")
            .unwrap();
        assert_eq!(ab.context, "A links to B");
    }

    #[test]
    fn parallel_edges_report_the_newest_context() {
        let (db, _) = setup();
        insert_edge(
            &db,
            &ParsedEdge {
                source_id: "a.md".into(),
                target_id: "b.md".into(),
                context: "second mention".into(),
                target_subpath: None,
            },
        )
        .unwrap();
        let kg = KnowledgeGraph::from_store(&db, false).unwrap();
        let paths = find_paths(&kg, "a.md", "b.md", 1);
        assert_eq!(paths[0].edges[0].context, "second mention");
    }
}
