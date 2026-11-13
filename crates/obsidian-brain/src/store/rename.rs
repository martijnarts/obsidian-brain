//! Renaming a node id in place.

use rusqlite::Connection;

use super::{Result, savepoint};

/// Atomically rewrite every row keyed on `old_id` to `new_id`: chunks (id
/// and node_id), edges (both ends), sync and the node itself. This is the
/// primitive behind `move_note`; updating in place keeps inbound edges,
/// which a delete-then-upsert would drop.
///
/// The node rowid does not change, so `nodes_fts`, `nodes_vec` and
/// `chunks_vec` stay valid. Absorbing a stub at the new id is the
/// caller's job (`migrate_stub_to_real`).
pub fn rename_node(conn: &Connection, old_id: &str, new_id: &str) -> Result<()> {
    if old_id == new_id {
        return Ok(());
    }
    savepoint(conn, "rename_node", || {
        // chunks.node_id references nodes(id), and no order of the updates
        // is valid midway, so check foreign keys at commit instead.
        conn.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        conn.execute(
            "UPDATE chunks SET id = (? || '::' || chunk_index), node_id = ?
       WHERE node_id = ?",
            [new_id, new_id, old_id],
        )?;
        conn.execute(
            "UPDATE edges SET source_id = ? WHERE source_id = ?",
            [new_id, old_id],
        )?;
        conn.execute(
            "UPDATE edges SET target_id = ? WHERE target_id = ?",
            [new_id, old_id],
        )?;
        conn.execute("UPDATE sync SET path = ? WHERE path = ?", [new_id, old_id])?;
        conn.execute("UPDATE nodes SET id = ? WHERE id = ?", [new_id, old_id])?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::super::db::open_db;
    use super::super::edges::{get_edges_by_source, get_edges_by_target, insert_edge};
    use super::super::nodes::{get_node, upsert_node};
    use super::super::sync::{get_all_sync_paths, get_sync_mtime, set_sync_mtime};
    use super::*;
    use crate::types::{ParsedEdge, ParsedNode};
    use serde_json::json;
    use std::collections::HashSet;

    fn node(db: &Connection, id: &str, title: &str) {
        upsert_node(
            db,
            &ParsedNode {
                id: id.into(),
                title: title.into(),
                content: String::new(),
                frontmatter: Default::default(),
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

    #[test]
    fn noop_when_ids_match() {
        let db = open_db(":memory:").unwrap();
        node(&db, "a.md", "a");
        rename_node(&db, "a.md", "a.md").unwrap();
        assert!(get_node(&db, "a.md").unwrap().is_some());
    }

    #[test]
    fn renames_the_node_and_keeps_its_fields() {
        let db = open_db(":memory:").unwrap();
        upsert_node(
            &db,
            &ParsedNode {
                id: "old.md".into(),
                title: "Old Title".into(),
                content: "Body".into(),
                frontmatter: json!({"tag": "x"}).as_object().unwrap().clone(),
            },
        )
        .unwrap();
        rename_node(&db, "old.md", "new.md").unwrap();
        assert!(get_node(&db, "old.md").unwrap().is_none());
        let moved = get_node(&db, "new.md").unwrap().unwrap();
        assert_eq!(moved.title, "Old Title");
        assert_eq!(moved.content, "Body");
        assert_eq!(
            serde_json::Value::Object(moved.frontmatter),
            json!({"tag": "x"})
        );
    }

    #[test]
    fn keeps_inbound_edges() {
        let db = open_db(":memory:").unwrap();
        for id in ["target.md", "a.md", "b.md"] {
            node(&db, id, id);
        }
        edge(&db, "a.md", "target.md", "x");
        edge(&db, "b.md", "target.md", "y");
        rename_node(&db, "target.md", "renamed.md").unwrap();
        assert!(get_edges_by_target(&db, "target.md").unwrap().is_empty());
        let inbound = get_edges_by_target(&db, "renamed.md").unwrap();
        let sources: HashSet<_> = inbound.iter().map(|e| e.source_id.as_str()).collect();
        let contexts: HashSet<_> = inbound.iter().map(|e| e.context.as_str()).collect();
        assert_eq!(sources, HashSet::from(["a.md", "b.md"]));
        assert_eq!(contexts, HashSet::from(["x", "y"]));
    }

    #[test]
    fn keeps_outbound_edges() {
        let db = open_db(":memory:").unwrap();
        for id in ["src.md", "x.md", "y.md"] {
            node(&db, id, id);
        }
        edge(&db, "src.md", "x.md", "a");
        edge(&db, "src.md", "y.md", "b");
        rename_node(&db, "src.md", "src-renamed.md").unwrap();
        assert!(get_edges_by_source(&db, "src.md").unwrap().is_empty());
        let out = get_edges_by_source(&db, "src-renamed.md").unwrap();
        let targets: HashSet<_> = out.iter().map(|e| e.target_id.as_str()).collect();
        assert_eq!(targets, HashSet::from(["x.md", "y.md"]));
    }

    #[test]
    fn rewrites_self_loops_on_both_ends() {
        let db = open_db(":memory:").unwrap();
        node(&db, "s.md", "S");
        edge(&db, "s.md", "s.md", "self");
        rename_node(&db, "s.md", "s-new.md").unwrap();
        assert!(get_edges_by_source(&db, "s.md").unwrap().is_empty());
        assert!(get_edges_by_target(&db, "s.md").unwrap().is_empty());
        let selfs = get_edges_by_source(&db, "s-new.md").unwrap();
        assert_eq!(selfs.len(), 1);
        assert_eq!(selfs[0].target_id, "s-new.md");
    }

    #[test]
    fn rewrites_chunk_ids_and_node_ids() {
        let db = open_db(":memory:").unwrap();
        node(&db, "chunky.md", "C");
        let insert = "INSERT INTO chunks (id, node_id, chunk_index, heading, heading_level, content, content_hash, start_line, end_line)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)";
        db.execute(
            insert,
            rusqlite::params![
                "chunky.md::0",
                "chunky.md",
                0,
                "H1",
                1,
                "body0",
                "hash0",
                1,
                10
            ],
        )
        .unwrap();
        db.execute(
            insert,
            rusqlite::params![
                "chunky.md::1",
                "chunky.md",
                1,
                "H2",
                2,
                "body1",
                "hash1",
                11,
                20
            ],
        )
        .unwrap();
        rename_node(&db, "chunky.md", "chunky-renamed.md").unwrap();
        let mut stmt = db
            .prepare("SELECT id, node_id, chunk_index FROM chunks ORDER BY chunk_index")
            .unwrap();
        let rows: Vec<(String, String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            rows,
            [
                ("chunky-renamed.md::0".into(), "chunky-renamed.md".into(), 0),
                ("chunky-renamed.md::1".into(), "chunky-renamed.md".into(), 1)
            ]
        );
    }

    #[test]
    fn rewrites_the_sync_path() {
        let db = open_db(":memory:").unwrap();
        node(&db, "tracked.md", "T");
        set_sync_mtime(&db, "tracked.md", 12345.0, None).unwrap();
        rename_node(&db, "tracked.md", "tracked-renamed.md").unwrap();
        assert_eq!(get_sync_mtime(&db, "tracked.md").unwrap(), None);
        assert_eq!(
            get_sync_mtime(&db, "tracked-renamed.md").unwrap(),
            Some(12345.0)
        );
        assert_eq!(get_all_sync_paths(&db).unwrap(), ["tracked-renamed.md"]);
    }

    #[test]
    fn works_inside_a_caller_transaction() {
        let mut db = open_db(":memory:").unwrap();
        node(&db, "a.md", "A");
        let tx = db.transaction().unwrap();
        rename_node(&tx, "a.md", "b.md").unwrap();
        tx.commit().unwrap();
        assert!(get_node(&db, "b.md").unwrap().is_some());
    }
}
