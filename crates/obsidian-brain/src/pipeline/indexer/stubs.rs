//! Stub nodes for wiki-link targets that do not exist yet.

use rusqlite::Connection;
use serde_json::{Map, Value};

use crate::store::Result;
use crate::store::nodes::{get_node, migrate_stub_to_real, upsert_node};
use crate::types::ParsedNode;

/// For each bare-stem stub (no `#` or `^`), look for a real note whose path
/// is `<stem>.md` or ends with `/<stem>.md`. When one exists, repoint the
/// stub's inbound edges to it and delete the stub.
///
/// Runs after [`materialise_stubs`], so the edges of the current pass are
/// stored before any migration.
pub fn resolve_forward_stubs(conn: &Connection) -> Result<()> {
    let mut stmt =
        conn.prepare("SELECT id FROM nodes WHERE json_extract(frontmatter, '$._stub') = 1")?;
    let stub_ids: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;

    for stub_id in stub_ids {
        let raw = stub_id.strip_prefix("_stub/").unwrap_or(&stub_id);
        let raw = raw.strip_suffix(".md").unwrap_or(raw);
        // Stubs with a fragment in their id predate the parser splitting
        // the fragment onto the edge. Orphan pruning removes them once their
        // edges are rewritten.
        if raw.contains('#') || raw.contains('^') {
            continue;
        }
        let want = format!("{raw}.md");
        let hit: Option<String> = conn
            .query_row(
                "SELECT id FROM nodes WHERE (id = ? OR id LIKE ?) AND (frontmatter IS NULL OR json_extract(frontmatter, '$._stub') IS NULL) LIMIT 1",
                [want.as_str(), &format!("%/{want}")],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                e => Err(e),
            })?;
        if let Some(real_id) = hit {
            migrate_stub_to_real(conn, &stub_id, &real_id)?;
        }
    }
    Ok(())
}

/// Create a stub node for every id in `stub_ids` that has no node yet.
/// Returns how many were created.
pub fn materialise_stubs<S: AsRef<str>>(conn: &Connection, stub_ids: &[S]) -> Result<usize> {
    let mut created = 0;
    for stub_id in stub_ids {
        let stub_id = stub_id.as_ref();
        if get_node(conn, stub_id)?.is_some() {
            continue;
        }
        let mut frontmatter = Map::new();
        frontmatter.insert("_stub".into(), Value::Bool(true));
        upsert_node(
            conn,
            &ParsedNode {
                id: stub_id.to_string(),
                title: stub_id.replacen("_stub/", "", 1).replacen(".md", "", 1),
                content: String::new(),
                frontmatter,
            },
        )?;
        created += 1;
    }
    Ok(created)
}
