//! Edge rows: one per outbound link, never de-duplicated.

use rusqlite::{Connection, params};
use serde::Serialize;

use super::Result;
use crate::types::ParsedEdge;

/// An edge row with its id.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredEdge {
    pub id: i64,
    pub source_id: String,
    pub target_id: String,
    pub context: String,
    pub target_subpath: Option<String>,
}

/// A distinct neighbour: its id and title, or its id again when no node
/// row exists for it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EdgeSummary {
    pub node_id: String,
    pub title: String,
}

/// Append an edge. `target_subpath` holds a `#heading` or `^block` suffix
/// without its `#` or `^`.
pub fn insert_edge(conn: &Connection, edge: &ParsedEdge) -> Result<()> {
    conn.execute(
        "INSERT INTO edges (source_id, target_id, context, target_subpath) VALUES (?, ?, ?, ?)",
        params![
            edge.source_id,
            edge.target_id,
            edge.context,
            edge.target_subpath
        ],
    )?;
    Ok(())
}

fn query_edges(conn: &Connection, sql: &str, node_id: &str) -> Result<Vec<StoredEdge>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([node_id], |r| {
        Ok(StoredEdge {
            id: r.get(0)?,
            source_id: r.get(1)?,
            target_id: r.get(2)?,
            context: r.get(3)?,
            target_subpath: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// All edges leaving `node_id`.
pub fn get_edges_by_source(conn: &Connection, node_id: &str) -> Result<Vec<StoredEdge>> {
    query_edges(
        conn,
        "SELECT id, source_id, target_id, context, target_subpath FROM edges WHERE source_id = ?",
        node_id,
    )
}

/// All edges arriving at `node_id`.
pub fn get_edges_by_target(conn: &Connection, node_id: &str) -> Result<Vec<StoredEdge>> {
    query_edges(
        conn,
        "SELECT id, source_id, target_id, context, target_subpath FROM edges WHERE target_id = ?",
        node_id,
    )
}

/// Number of edges leaving `node_id`.
pub fn count_edges_by_source(conn: &Connection, node_id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) as cnt FROM edges WHERE source_id = ?",
        [node_id],
        |r| r.get(0),
    )?)
}

/// Number of edges arriving at `node_id`.
pub fn count_edges_by_target(conn: &Connection, node_id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) as cnt FROM edges WHERE target_id = ?",
        [node_id],
        |r| r.get(0),
    )?)
}

fn query_summaries(conn: &Connection, sql: &str, node_id: &str) -> Result<Vec<EdgeSummary>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([node_id], |r| {
        let id: String = r.get(0)?;
        let title: Option<String> = r.get(1)?;
        Ok(EdgeSummary {
            title: title.unwrap_or_else(|| id.clone()),
            node_id: id,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Distinct outgoing neighbours.
pub fn get_edge_summaries_by_source(conn: &Connection, node_id: &str) -> Result<Vec<EdgeSummary>> {
    query_summaries(
        conn,
        "SELECT DISTINCT e.target_id, n.title
       FROM edges e
       LEFT JOIN nodes n ON n.id = e.target_id
       WHERE e.source_id = ?",
        node_id,
    )
}

/// Distinct incoming neighbours (backlinks).
pub fn get_edge_summaries_by_target(conn: &Connection, node_id: &str) -> Result<Vec<EdgeSummary>> {
    query_summaries(
        conn,
        "SELECT DISTINCT e.source_id, n.title
       FROM edges e
       LEFT JOIN nodes n ON n.id = e.source_id
       WHERE e.target_id = ?",
        node_id,
    )
}

/// Delete every edge leaving `node_id`. The indexer does this before it
/// re-emits a note's links.
pub fn delete_edges_by_source(conn: &Connection, node_id: &str) -> Result<()> {
    conn.execute("DELETE FROM edges WHERE source_id = ?", [node_id])?;
    Ok(())
}

/// Delete every edge arriving at `node_id`.
pub fn delete_edges_by_target(conn: &Connection, node_id: &str) -> Result<()> {
    conn.execute("DELETE FROM edges WHERE target_id = ?", [node_id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::db::open_db;
    use super::super::nodes::upsert_node;
    use super::*;
    use crate::types::ParsedNode;

    fn setup() -> Connection {
        let db = open_db(":memory:").unwrap();
        for (id, title) in [("a.md", "Alpha"), ("b.md", "Beta"), ("c.md", "Gamma")] {
            upsert_node(
                &db,
                &ParsedNode {
                    id: id.into(),
                    title: title.into(),
                    content: String::new(),
                    frontmatter: Default::default(),
                },
            )
            .unwrap();
        }
        db
    }

    fn edge(s: &str, t: &str, c: &str) -> ParsedEdge {
        ParsedEdge {
            source_id: s.into(),
            target_id: t.into(),
            context: c.into(),
            target_subpath: None,
        }
    }

    #[test]
    fn inserts_and_retrieves_edges_by_source() {
        let db = setup();
        insert_edge(&db, &edge("a.md", "b.md", "A links to B")).unwrap();
        let edges = get_edges_by_source(&db, "a.md").unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target_id, "b.md");
        assert_eq!(edges[0].context, "A links to B");
    }

    #[test]
    fn stores_the_target_subpath() {
        let db = setup();
        let mut e = edge("a.md", "b.md", "x");
        e.target_subpath = Some("Heading".into());
        insert_edge(&db, &e).unwrap();
        assert_eq!(
            get_edges_by_source(&db, "a.md").unwrap()[0]
                .target_subpath
                .as_deref(),
            Some("Heading")
        );
    }

    #[test]
    fn allows_multiple_edges_between_the_same_pair() {
        let db = setup();
        insert_edge(&db, &edge("a.md", "b.md", "First mention")).unwrap();
        insert_edge(&db, &edge("a.md", "b.md", "Second mention")).unwrap();
        assert_eq!(get_edges_by_source(&db, "a.md").unwrap().len(), 2);
    }

    #[test]
    fn retrieves_backlinks() {
        let db = setup();
        insert_edge(&db, &edge("a.md", "b.md", "link")).unwrap();
        let back = get_edges_by_target(&db, "b.md").unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].source_id, "a.md");
    }

    #[test]
    fn counts_edges_for_a_node() {
        let db = setup();
        insert_edge(&db, &edge("a.md", "b.md", "link 1")).unwrap();
        insert_edge(&db, &edge("a.md", "c.md", "link 2")).unwrap();
        insert_edge(&db, &edge("b.md", "a.md", "backlink")).unwrap();
        assert_eq!(count_edges_by_source(&db, "a.md").unwrap(), 2);
        assert_eq!(count_edges_by_target(&db, "a.md").unwrap(), 1);
    }

    #[test]
    fn gets_edge_summaries() {
        let db = setup();
        insert_edge(&db, &edge("a.md", "b.md", "long paragraph...")).unwrap();
        insert_edge(&db, &edge("c.md", "a.md", "another paragraph...")).unwrap();
        let out = get_edge_summaries_by_source(&db, "a.md").unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "Beta");
        let inn = get_edge_summaries_by_target(&db, "a.md").unwrap();
        assert_eq!(inn.len(), 1);
        assert_eq!(inn[0].title, "Gamma");
    }

    #[test]
    fn summary_dedupes_multiple_edges() {
        let db = setup();
        insert_edge(&db, &edge("a.md", "b.md", "first")).unwrap();
        insert_edge(&db, &edge("a.md", "b.md", "second")).unwrap();
        assert_eq!(get_edge_summaries_by_source(&db, "a.md").unwrap().len(), 1);
    }

    #[test]
    fn summary_falls_back_to_target_id() {
        let db = setup();
        insert_edge(&db, &edge("a.md", "dangling.md", "x")).unwrap();
        assert_eq!(
            get_edge_summaries_by_source(&db, "a.md").unwrap()[0].title,
            "dangling.md"
        );
    }

    #[test]
    fn delete_by_source_removes_outgoing_only() {
        let db = setup();
        insert_edge(&db, &edge("a.md", "b.md", "out")).unwrap();
        insert_edge(&db, &edge("c.md", "a.md", "in")).unwrap();
        delete_edges_by_source(&db, "a.md").unwrap();
        assert!(get_edges_by_source(&db, "a.md").unwrap().is_empty());
        assert_eq!(get_edges_by_target(&db, "a.md").unwrap().len(), 1);
    }

    #[test]
    fn delete_by_target_removes_incoming_only() {
        let db = setup();
        insert_edge(&db, &edge("a.md", "b.md", "out")).unwrap();
        insert_edge(&db, &edge("c.md", "a.md", "in")).unwrap();
        delete_edges_by_target(&db, "a.md").unwrap();
        assert!(get_edges_by_target(&db, "a.md").unwrap().is_empty());
        assert_eq!(get_edges_by_source(&db, "a.md").unwrap().len(), 1);
    }
}
