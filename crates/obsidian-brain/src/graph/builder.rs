//! The link graph, built from the store.
//!
//! Iteration orders follow graphology, because tool output and score ties
//! depend on them: nodes in insertion order, neighbours in the order of
//! their first edge, edges in insertion order.

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

use crate::store::Result;
use crate::store::edges::get_edges_by_source;
use crate::store::nodes::{all_node_ids, get_node, is_stub};
use crate::types::Frontmatter;

#[derive(Debug, Clone, PartialEq)]
pub struct GraphNode {
    pub id: String,
    pub title: String,
    pub frontmatter: Frontmatter,
}

/// A directed multigraph of notes and their links.
#[derive(Debug, Clone, Default)]
pub struct KnowledgeGraph {
    nodes: Vec<GraphNode>,
    index: HashMap<String, usize>,
    /// Distinct out-neighbours of each node, in first-edge order.
    out: Vec<Vec<usize>>,
    /// Distinct in-neighbours of each node, in first-edge order.
    inn: Vec<Vec<usize>>,
    /// The context of every edge per (source, target), in insertion order.
    contexts: HashMap<(usize, usize), Vec<String>>,
    /// Every edge in insertion order.
    edges: Vec<(usize, usize)>,
}

impl KnowledgeGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the graph from the store: every node, then every edge whose
    /// target is a node (dangling edges are dropped).
    ///
    /// With `include_stubs` false, unresolved link targets
    /// (`_stub: true`) and their edges stay out. Stubs are popular but
    /// empty, and would otherwise flood centrality.
    pub fn from_store(conn: &Connection, include_stubs: bool) -> Result<Self> {
        let mut g = Self::new();
        let ids = all_node_ids(conn)?;
        let mut skipped = HashSet::new();
        for id in &ids {
            let Some(node) = get_node(conn, id)? else {
                continue;
            };
            if !include_stubs && is_stub(&node.frontmatter) {
                skipped.insert(id.as_str());
                continue;
            }
            g.add_node(GraphNode {
                id: node.id,
                title: node.title,
                frontmatter: node.frontmatter,
            });
        }
        for id in &ids {
            if skipped.contains(id.as_str()) {
                continue;
            }
            for edge in get_edges_by_source(conn, id)? {
                if g.has_node(&edge.target_id) {
                    g.add_edge(&edge.source_id, &edge.target_id, &edge.context);
                }
            }
        }
        Ok(g)
    }

    /// Add a node. A node with an existing id is ignored.
    pub fn add_node(&mut self, node: GraphNode) {
        if self.index.contains_key(&node.id) {
            return;
        }
        self.index.insert(node.id.clone(), self.nodes.len());
        self.nodes.push(node);
        self.out.push(Vec::new());
        self.inn.push(Vec::new());
    }

    /// Add a directed edge. Returns false when either end is missing.
    pub fn add_edge(&mut self, source: &str, target: &str, context: &str) -> bool {
        let (Some(&s), Some(&t)) = (self.index.get(source), self.index.get(target)) else {
            return false;
        };
        let list = self.contexts.entry((s, t)).or_default();
        if list.is_empty() {
            self.out[s].push(t);
            self.inn[t].push(s);
        }
        list.push(context.to_string());
        self.edges.push((s, t));
        true
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn has_node(&self, id: &str) -> bool {
        self.index.contains_key(id)
    }

    pub fn node(&self, id: &str) -> Option<&GraphNode> {
        self.index.get(id).map(|&i| &self.nodes[i])
    }

    /// Every node, in insertion order.
    pub fn nodes(&self) -> &[GraphNode] {
        &self.nodes
    }

    /// The title of `id`, or `id` itself when the node is missing.
    pub fn node_title(&self, id: &str) -> String {
        self.node(id)
            .map_or_else(|| id.to_string(), |n| n.title.clone())
    }

    /// Distinct targets of edges leaving `id` (empty when missing).
    pub fn out_neighbors(&self, id: &str) -> Vec<&str> {
        self.index
            .get(id)
            .map(|&i| {
                self.out[i]
                    .iter()
                    .map(|&j| self.nodes[j].id.as_str())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Distinct sources of edges arriving at `id` (empty when missing).
    pub fn in_neighbors(&self, id: &str) -> Vec<&str> {
        self.index
            .get(id)
            .map(|&i| {
                self.inn[i]
                    .iter()
                    .map(|&j| self.nodes[j].id.as_str())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// True when at least one edge runs from `source` to `target`.
    pub fn has_edge(&self, source: &str, target: &str) -> bool {
        self.edge_contexts(source, target).is_some()
    }

    /// The contexts of the edges from `source` to `target`, in insertion
    /// order, or `None` when there is no such edge.
    pub fn edge_contexts(&self, source: &str, target: &str) -> Option<&[String]> {
        let s = *self.index.get(source)?;
        let t = *self.index.get(target)?;
        self.contexts.get(&(s, t)).map(Vec::as_slice)
    }

    pub(crate) fn out_indices(&self, i: usize) -> &[usize] {
        &self.out[i]
    }

    pub(crate) fn in_indices(&self, i: usize) -> &[usize] {
        &self.inn[i]
    }

    pub(crate) fn index_of(&self, id: &str) -> Option<usize> {
        self.index.get(id).copied()
    }

    /// Collapse into an undirected simple graph, for PageRank and
    /// betweenness.
    pub fn to_undirected(&self) -> UndirectedGraph {
        let mut u = UndirectedGraph::new();
        for n in &self.nodes {
            u.add_node(&n.id, &n.title);
        }
        for &(s, t) in &self.edges {
            u.add_edge_at(s, t);
        }
        u
    }
}

/// An undirected simple graph. Self-loops are kept, once.
#[derive(Debug, Clone, Default)]
pub struct UndirectedGraph {
    ids: Vec<String>,
    titles: Vec<String>,
    index: HashMap<String, usize>,
    /// Neighbours of each node in first-edge order; a self-loop appears once.
    adj: Vec<Vec<usize>>,
    self_loops: Vec<bool>,
    edge_set: HashSet<(usize, usize)>,
    /// Every edge in insertion order.
    edges: Vec<(usize, usize)>,
}

impl UndirectedGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a node. A node with an existing id is ignored.
    pub fn add_node(&mut self, id: &str, title: &str) {
        if self.index.contains_key(id) {
            return;
        }
        self.index.insert(id.to_string(), self.ids.len());
        self.ids.push(id.to_string());
        self.titles.push(title.to_string());
        self.adj.push(Vec::new());
        self.self_loops.push(false);
    }

    /// Add an edge unless one already joins the two nodes. Returns false
    /// when either end is missing.
    pub fn add_edge(&mut self, a: &str, b: &str) -> bool {
        let (Some(&a), Some(&b)) = (self.index.get(a), self.index.get(b)) else {
            return false;
        };
        self.add_edge_at(a, b);
        true
    }

    fn add_edge_at(&mut self, a: usize, b: usize) {
        if !self.edge_set.insert((a.min(b), a.max(b))) {
            return;
        }
        self.edges.push((a, b));
        if a == b {
            self.adj[a].push(a);
            self.self_loops[a] = true;
        } else {
            self.adj[a].push(b);
            self.adj[b].push(a);
        }
    }

    /// Number of nodes.
    pub fn order(&self) -> usize {
        self.ids.len()
    }

    /// Number of edges.
    pub fn size(&self) -> usize {
        self.edges.len()
    }

    pub fn has_node(&self, id: &str) -> bool {
        self.index.contains_key(id)
    }

    pub fn has_edge(&self, a: &str, b: &str) -> bool {
        match (self.index.get(a), self.index.get(b)) {
            (Some(&a), Some(&b)) => self.edge_set.contains(&(a.min(b), a.max(b))),
            _ => false,
        }
    }

    /// Node ids in insertion order.
    pub fn node_ids(&self) -> &[String] {
        &self.ids
    }

    /// The title of `id`, or `id` itself when the node is missing.
    pub fn node_title(&self, id: &str) -> String {
        self.index
            .get(id)
            .map_or_else(|| id.to_string(), |&i| self.titles[i].clone())
    }

    /// Neighbours of `id`, itself included when it has a self-loop.
    pub fn neighbors(&self, id: &str) -> Vec<&str> {
        self.index
            .get(id)
            .map(|&i| self.adj[i].iter().map(|&j| self.ids[j].as_str()).collect())
            .unwrap_or_default()
    }

    /// The degree of `id`; a self-loop counts twice, as in graphology.
    pub fn degree(&self, id: &str) -> usize {
        self.index.get(id).map_or(0, |&i| self.degree_at(i))
    }

    pub(crate) fn degree_at(&self, i: usize) -> usize {
        self.adj[i].len() + usize::from(self.self_loops[i])
    }

    pub(crate) fn adj_at(&self, i: usize) -> &[usize] {
        &self.adj[i]
    }

    pub(crate) fn edges_raw(&self) -> &[(usize, usize)] {
        &self.edges
    }

    pub(crate) fn title_at(&self, i: usize) -> &str {
        &self.titles[i]
    }

    pub(crate) fn id_at(&self, i: usize) -> &str {
        &self.ids[i]
    }

    pub(crate) fn index_of(&self, id: &str) -> Option<usize> {
        self.index.get(id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::db::open_db;
    use crate::store::edges::insert_edge;
    use crate::store::nodes::upsert_node;
    use crate::types::{ParsedEdge, ParsedNode};
    use serde_json::{Value, json};

    fn add_node(db: &Connection, id: &str, title: &str, fm: Value) {
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

    fn add_edge(db: &Connection, s: &str, t: &str, c: &str) {
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

    fn setup() -> Connection {
        let db = open_db(":memory:").unwrap();
        for (id, title) in [("a.md", "A"), ("b.md", "B"), ("c.md", "C"), ("d.md", "D")] {
            add_node(&db, id, title, json!({}));
        }
        add_edge(&db, "a.md", "b.md", "A links to B");
        add_edge(&db, "b.md", "c.md", "B links to C");
        add_edge(&db, "a.md", "c.md", "A links to C");
        db
    }

    #[test]
    fn loads_all_nodes_and_edges() {
        let kg = KnowledgeGraph::from_store(&setup(), false).unwrap();
        assert_eq!(kg.node_count(), 4);
        assert_eq!(kg.edge_count(), 3);
    }

    #[test]
    fn node_title_falls_back_to_id() {
        let kg = KnowledgeGraph::from_store(&setup(), false).unwrap();
        assert_eq!(kg.node_title("a.md"), "A");
        assert_eq!(kg.node_title("missing.md"), "missing.md");
    }

    #[test]
    fn neighbors_reflect_directed_edges() {
        let kg = KnowledgeGraph::from_store(&setup(), false).unwrap();
        assert_eq!(kg.out_neighbors("a.md"), ["b.md", "c.md"]);
        assert_eq!(kg.in_neighbors("c.md"), ["a.md", "b.md"]);
        assert!(kg.out_neighbors("d.md").is_empty());
        assert!(kg.in_neighbors("d.md").is_empty());
    }

    #[test]
    fn to_undirected_collapses_to_a_simple_graph() {
        let db = setup();
        add_edge(&db, "b.md", "a.md", "back");
        add_edge(&db, "a.md", "b.md", "again");
        let kg = KnowledgeGraph::from_store(&db, false).unwrap();
        let u = kg.to_undirected();
        assert_eq!(u.order(), 4);
        assert_eq!(u.size(), 3);
        assert!(u.has_edge("a.md", "b.md") && u.has_edge("b.md", "a.md"));
    }

    #[test]
    fn self_loops_count_twice_in_degree() {
        let mut u = UndirectedGraph::new();
        u.add_node("a", "a");
        u.add_node("b", "b");
        u.add_edge("a", "b");
        u.add_edge("a", "a");
        assert_eq!(u.degree("a"), 3);
        assert_eq!(u.neighbors("a"), ["b", "a"]);
    }

    #[test]
    fn dangling_edges_are_dropped() {
        let db = setup();
        add_edge(&db, "a.md", "ghost.md", "x");
        assert_eq!(
            KnowledgeGraph::from_store(&db, false).unwrap().edge_count(),
            3
        );
    }

    #[test]
    fn excludes_stubs_by_default() {
        let db = setup();
        add_node(&db, "_stub/Ghost.md", "Ghost", json!({"_stub": true}));
        add_edge(&db, "a.md", "_stub/Ghost.md", "A → ghost");
        let kg = KnowledgeGraph::from_store(&db, false).unwrap();
        assert!(!kg.has_node("_stub/Ghost.md"));
        assert_eq!(kg.edge_count(), 3);
        assert!(kg.has_node("a.md"));
        assert_eq!(kg.node_count(), 4);
    }

    #[test]
    fn include_stubs_keeps_them() {
        let db = setup();
        add_node(&db, "_stub/Ghost.md", "Ghost", json!({"_stub": true}));
        add_edge(&db, "a.md", "_stub/Ghost.md", "A → ghost");
        let kg = KnowledgeGraph::from_store(&db, true).unwrap();
        assert!(kg.has_node("_stub/Ghost.md"));
        assert_eq!(kg.node_count(), 5);
        assert_eq!(kg.edge_count(), 4);
    }

    #[test]
    fn edges_from_a_filtered_stub_are_dropped() {
        let db = setup();
        add_node(&db, "_stub/Hub.md", "Hub", json!({"_stub": true}));
        add_edge(&db, "_stub/Hub.md", "a.md", "stub → a");
        let kg = KnowledgeGraph::from_store(&db, false).unwrap();
        assert!(!kg.has_node("_stub/Hub.md"));
        assert_eq!(kg.edge_count(), 3);
    }
}
