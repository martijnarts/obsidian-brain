//! Node rows, with the FTS5 index and the embeddings kept in step.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use super::Result;
use super::edges::{count_edges_by_target, delete_edges_by_source, delete_edges_by_target};
use super::sync::delete_sync_path;
use crate::types::{Frontmatter, ParsedNode};

/// A node row with its SQLite rowid, which keys `nodes_fts` and `nodes_vec`.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredNode {
    pub rowid: i64,
    pub id: String,
    pub title: String,
    pub content: String,
    pub frontmatter: Frontmatter,
}

impl StoredNode {
    pub fn into_parsed(self) -> ParsedNode {
        ParsedNode {
            id: self.id,
            title: self.title,
            content: self.content,
            frontmatter: self.frontmatter,
        }
    }
}

/// One note without its content, as `allNotes` returns it.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteSummary {
    pub id: String,
    pub title: String,
    pub frontmatter: Frontmatter,
}

/// True when the frontmatter marks an unresolved link target
/// (`_stub: true`).
pub fn is_stub(frontmatter: &Frontmatter) -> bool {
    frontmatter.get("_stub") == Some(&Value::Bool(true))
}

fn parse_frontmatter(text: &str) -> Result<Frontmatter> {
    Ok(serde_json::from_str(text)?)
}

/// Insert or replace a node. FTS5 with `content='nodes'` needs an explicit
/// 'delete' with the OLD values before the new row goes in, or the index
/// drifts.
pub fn upsert_node(conn: &Connection, node: &ParsedNode) -> Result<()> {
    let existing: Option<(i64, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT rowid, title, content FROM nodes WHERE id = ?",
            [&node.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;

    if let Some((rowid, title, content)) = existing {
        conn.execute(
            "INSERT INTO nodes_fts(nodes_fts, rowid, title, content) VALUES('delete', ?, ?, ?)",
            params![
                rowid,
                title.unwrap_or_default(),
                content.unwrap_or_default()
            ],
        )?;
    }

    conn.execute(
        "INSERT INTO nodes (id, title, content, frontmatter)
       VALUES (?, ?, ?, ?)
       ON CONFLICT(id) DO UPDATE SET
         title = excluded.title,
         content = excluded.content,
         frontmatter = excluded.frontmatter",
        params![
            node.id,
            node.title,
            node.content,
            serde_json::to_string(&node.frontmatter)?
        ],
    )?;

    let rowid: i64 = conn.query_row("SELECT rowid FROM nodes WHERE id = ?", [&node.id], |r| {
        r.get(0)
    })?;

    conn.execute(
        "INSERT INTO nodes_fts(rowid, title, content) VALUES(?, ?, ?)",
        params![rowid, node.title, node.content],
    )?;
    Ok(())
}

/// Fetch a node by id, with its rowid.
pub fn get_node(conn: &Connection, id: &str) -> Result<Option<StoredNode>> {
    let row: Option<(i64, String, String, Option<String>, String)> = conn
        .query_row(
            "SELECT rowid, id, title, content, frontmatter FROM nodes WHERE id = ?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((rowid, id, title, content, frontmatter)) = row else {
        return Ok(None);
    };
    Ok(Some(StoredNode {
        rowid,
        id,
        title,
        content: content.unwrap_or_default(),
        frontmatter: parse_frontmatter(&frontmatter)?,
    }))
}

/// Every node id in the database.
pub fn all_node_ids(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM nodes")?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Every real note (stubs excluded) with its parsed frontmatter, in id order.
pub fn all_notes(conn: &Connection) -> Result<Vec<NoteSummary>> {
    let mut stmt = conn.prepare("SELECT id, title, frontmatter FROM nodes ORDER BY id")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, title, fm) = row?;
        let frontmatter = parse_frontmatter(&fm)?;
        if !is_stub(&frontmatter) {
            out.push(NoteSummary {
                id,
                title,
                frontmatter,
            });
        }
    }
    Ok(out)
}

/// Remove a node and everything that references it: the FTS5 row, its
/// embedding, its chunks and their vectors, its edges in both directions
/// and its sync row.
pub fn delete_node(conn: &Connection, id: &str) -> Result<()> {
    let row: Option<(i64, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT rowid, title, content FROM nodes WHERE id = ?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;

    if let Some((rowid, title, content)) = row {
        conn.execute(
            "INSERT INTO nodes_fts(nodes_fts, rowid, title, content) VALUES('delete', ?, ?, ?)",
            params![
                rowid,
                title.unwrap_or_default(),
                content.unwrap_or_default()
            ],
        )?;
        conn.execute("DELETE FROM nodes_vec WHERE rowid = ?", [rowid])?;
    }

    // chunks_vec is a virtual table without a cascade, so clear it first.
    super::chunks::delete_chunks_for_node(conn, id)?;

    conn.execute("DELETE FROM nodes WHERE id = ?", [id])?;
    delete_edges_by_source(conn, id)?;
    delete_edges_by_target(conn, id)?;
    delete_sync_path(conn, id)?;
    Ok(())
}

/// Delete every stub in `candidate_ids` that has no inbound edges. Ids that
/// are not under `_stub/`, are missing, or are not marked `_stub: true`
/// are skipped. Returns the number pruned.
pub fn prune_orphan_stubs<S: AsRef<str>>(conn: &Connection, candidate_ids: &[S]) -> Result<usize> {
    let mut pruned = 0;
    for id in candidate_ids {
        let id = id.as_ref();
        if !id.starts_with("_stub/") {
            continue;
        }
        let Some(node) = get_node(conn, id)? else {
            continue;
        };
        if !is_stub(&node.frontmatter) {
            continue;
        }
        if count_edges_by_target(conn, id)? != 0 {
            continue;
        }
        delete_node(conn, id)?;
        pruned += 1;
    }
    Ok(pruned)
}

/// Prune every stub in the database that has no inbound edges.
pub fn prune_all_orphan_stubs(conn: &Connection) -> Result<usize> {
    let mut stmt =
        conn.prepare("SELECT id FROM nodes WHERE json_extract(frontmatter, '$._stub') = 1")?;
    let ids: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    prune_orphan_stubs(conn, &ids)
}

/// When a real note appears for an existing stub, repoint the stub's
/// inbound edges to the real node and delete the stub. A no-op when the
/// stub is missing or not marked `_stub: true`.
pub fn migrate_stub_to_real(conn: &Connection, stub_id: &str, real_id: &str) -> Result<()> {
    let Some(node) = get_node(conn, stub_id)? else {
        return Ok(());
    };
    if !is_stub(&node.frontmatter) {
        return Ok(());
    }
    conn.execute(
        "UPDATE edges SET target_id = ? WHERE target_id = ?",
        [real_id, stub_id],
    )?;
    delete_node(conn, stub_id)
}

#[cfg(test)]
mod tests {
    use super::super::db::open_db;
    use super::super::edges::{get_edges_by_source, get_edges_by_target, insert_edge};
    use super::super::fulltext::search_full_text;
    use super::*;
    use crate::types::ParsedEdge;
    use serde_json::json;

    fn node(id: &str, title: &str, content: &str, fm: Value) -> ParsedNode {
        ParsedNode {
            id: id.into(),
            title: title.into(),
            content: content.into(),
            frontmatter: fm.as_object().cloned().unwrap_or_default(),
        }
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
    fn upserts_and_retrieves_nodes() {
        let db = open_db(":memory:").unwrap();
        upsert_node(
            &db,
            &node("test.md", "Test", "Hello world", json!({"type": "test"})),
        )
        .unwrap();
        let n = get_node(&db, "test.md").unwrap().unwrap();
        assert_eq!(n.title, "Test");
        assert_eq!(Value::Object(n.frontmatter), json!({"type": "test"}));
    }

    #[test]
    fn updates_existing_nodes_on_re_upsert() {
        let db = open_db(":memory:").unwrap();
        upsert_node(&db, &node("test.md", "Original", "v1", json!({}))).unwrap();
        upsert_node(
            &db,
            &node("test.md", "Updated", "v2", json!({"key": "value"})),
        )
        .unwrap();
        let n = get_node(&db, "test.md").unwrap().unwrap();
        assert_eq!(n.title, "Updated");
        assert_eq!(n.content, "v2");
        assert_eq!(Value::Object(n.frontmatter), json!({"key": "value"}));
    }

    #[test]
    fn lists_all_node_ids() {
        let db = open_db(":memory:").unwrap();
        upsert_node(&db, &node("a.md", "A", "", json!({}))).unwrap();
        upsert_node(&db, &node("b.md", "B", "", json!({}))).unwrap();
        let ids = all_node_ids(&db).unwrap();
        assert!(ids.contains(&"a.md".into()) && ids.contains(&"b.md".into()));
    }

    #[test]
    fn all_notes_skips_stubs_and_sorts_by_id() {
        let db = open_db(":memory:").unwrap();
        upsert_node(&db, &node("b.md", "B", "", json!({}))).unwrap();
        upsert_node(&db, &node("a.md", "A", "", json!({"tags": ["x"]}))).unwrap();
        upsert_node(&db, &node("_stub/c.md", "C", "", json!({"_stub": true}))).unwrap();
        let notes = all_notes(&db).unwrap();
        let ids: Vec<_> = notes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["a.md", "b.md"]);
    }

    #[test]
    fn deletes_a_node_and_cascades_to_edges() {
        let db = open_db(":memory:").unwrap();
        upsert_node(&db, &node("a.md", "A", "", json!({}))).unwrap();
        upsert_node(&db, &node("b.md", "B", "", json!({}))).unwrap();
        insert_edge(&db, &edge("a.md", "b.md", "link")).unwrap();
        delete_node(&db, "a.md").unwrap();
        assert!(get_node(&db, "a.md").unwrap().is_none());
        assert!(get_edges_by_source(&db, "a.md").unwrap().is_empty());
    }

    #[test]
    fn performs_full_text_search_via_fts5() {
        let db = open_db(":memory:").unwrap();
        upsert_node(
            &db,
            &node(
                "test.md",
                "Widget Theory",
                "A framework for understanding component interactions",
                json!({}),
            ),
        )
        .unwrap();
        let results = search_full_text(&db, "framework component", 20).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].node_id, "test.md");
    }

    #[test]
    fn full_text_search_returns_snippets() {
        let db = open_db(":memory:").unwrap();
        upsert_node(
            &db,
            &node(
                "test.md",
                "Widget Theory",
                "A framework for understanding component interactions in complex distributed systems",
                json!({}),
            ),
        )
        .unwrap();
        let results = search_full_text(&db, "framework component", 20).unwrap();
        assert!(!results.is_empty());
        assert!(results[0].excerpt.contains("framework"));
    }

    #[test]
    fn full_text_search_is_resynced_after_re_upsert() {
        let db = open_db(":memory:").unwrap();
        upsert_node(
            &db,
            &node(
                "test.md",
                "Original Title",
                "original content about foobar",
                json!({}),
            ),
        )
        .unwrap();
        upsert_node(
            &db,
            &node(
                "test.md",
                "New Title",
                "updated content about quux",
                json!({}),
            ),
        )
        .unwrap();
        assert!(search_full_text(&db, "foobar", 20).unwrap().is_empty());
        let results = search_full_text(&db, "quux", 20).unwrap();
        assert_eq!(results[0].node_id, "test.md");
    }

    #[test]
    fn prune_deletes_a_stub_with_zero_inbound_edges() {
        let db = open_db(":memory:").unwrap();
        upsert_node(
            &db,
            &node("_stub/orphan.md", "Orphan", "", json!({"_stub": true})),
        )
        .unwrap();
        assert_eq!(prune_orphan_stubs(&db, &["_stub/orphan.md"]).unwrap(), 1);
        assert!(get_node(&db, "_stub/orphan.md").unwrap().is_none());
    }

    #[test]
    fn prune_leaves_a_linked_stub_alone() {
        let db = open_db(":memory:").unwrap();
        upsert_node(
            &db,
            &node("_stub/linked.md", "Linked", "", json!({"_stub": true})),
        )
        .unwrap();
        upsert_node(&db, &node("source.md", "Source", "", json!({}))).unwrap();
        insert_edge(&db, &edge("source.md", "_stub/linked.md", "ref")).unwrap();
        assert_eq!(prune_orphan_stubs(&db, &["_stub/linked.md"]).unwrap(), 0);
        assert!(get_node(&db, "_stub/linked.md").unwrap().is_some());
    }

    #[test]
    fn prune_ignores_ids_outside_stub_dir() {
        let db = open_db(":memory:").unwrap();
        upsert_node(
            &db,
            &node("regular.md", "Regular", "", json!({"_stub": true})),
        )
        .unwrap();
        assert_eq!(prune_orphan_stubs(&db, &["regular.md"]).unwrap(), 0);
        assert!(get_node(&db, "regular.md").unwrap().is_some());
    }

    #[test]
    fn prune_ignores_nodes_without_stub_flag() {
        let db = open_db(":memory:").unwrap();
        upsert_node(&db, &node("_stub/notstub.md", "Not a stub", "", json!({}))).unwrap();
        assert_eq!(prune_orphan_stubs(&db, &["_stub/notstub.md"]).unwrap(), 0);
        assert!(get_node(&db, "_stub/notstub.md").unwrap().is_some());
    }

    #[test]
    fn prune_all_sweeps_orphan_stubs() {
        let db = open_db(":memory:").unwrap();
        upsert_node(&db, &node("_stub/a.md", "A", "", json!({"_stub": true}))).unwrap();
        upsert_node(&db, &node("_stub/b.md", "B", "", json!({"_stub": true}))).unwrap();
        upsert_node(&db, &node("source.md", "Source", "", json!({}))).unwrap();
        insert_edge(&db, &edge("source.md", "_stub/b.md", "ref")).unwrap();
        assert_eq!(prune_all_orphan_stubs(&db).unwrap(), 1);
        assert!(get_node(&db, "_stub/a.md").unwrap().is_none());
        assert!(get_node(&db, "_stub/b.md").unwrap().is_some());
    }

    #[test]
    fn upsert_handles_empty_frontmatter_and_content() {
        let db = open_db(":memory:").unwrap();
        upsert_node(&db, &node("no-fm.md", "No FM", "", json!({}))).unwrap();
        let n = get_node(&db, "no-fm.md").unwrap().unwrap();
        assert!(n.frontmatter.is_empty());
        assert_eq!(n.content, "");
    }

    #[test]
    fn migrate_stub_repoints_inbound_edges_and_deletes_stub() {
        let db = open_db(":memory:").unwrap();
        upsert_node(
            &db,
            &node("_stub/note.md", "Stub", "", json!({"_stub": true})),
        )
        .unwrap();
        upsert_node(&db, &node("real/note.md", "Real", "", json!({}))).unwrap();
        upsert_node(&db, &node("source.md", "Source", "", json!({}))).unwrap();
        insert_edge(&db, &edge("source.md", "_stub/note.md", "ref")).unwrap();
        migrate_stub_to_real(&db, "_stub/note.md", "real/note.md").unwrap();
        assert!(get_node(&db, "_stub/note.md").unwrap().is_none());
        let inbound = get_edges_by_target(&db, "real/note.md").unwrap();
        assert_eq!(inbound.len(), 1);
        assert_eq!(inbound[0].source_id, "source.md");
    }

    #[test]
    fn migrate_stub_is_a_noop_for_missing_stub() {
        let db = open_db(":memory:").unwrap();
        upsert_node(&db, &node("real/note.md", "Real", "", json!({}))).unwrap();
        migrate_stub_to_real(&db, "_stub/missing.md", "real/note.md").unwrap();
    }

    #[test]
    fn migrate_stub_is_a_noop_for_non_stub() {
        let db = open_db(":memory:").unwrap();
        upsert_node(&db, &node("_stub/notstub.md", "Not a stub", "", json!({}))).unwrap();
        upsert_node(&db, &node("real/note.md", "Real", "", json!({}))).unwrap();
        upsert_node(&db, &node("source.md", "Source", "", json!({}))).unwrap();
        insert_edge(&db, &edge("source.md", "_stub/notstub.md", "ref")).unwrap();
        migrate_stub_to_real(&db, "_stub/notstub.md", "real/note.md").unwrap();
        assert!(get_node(&db, "_stub/notstub.md").unwrap().is_some());
        assert_eq!(
            get_edges_by_target(&db, "_stub/notstub.md").unwrap().len(),
            1
        );
        assert!(get_edges_by_target(&db, "real/note.md").unwrap().is_empty());
    }
}
