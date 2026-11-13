//! Per-file sync state: the mtime each vault path had when it was indexed.

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, params};

use super::Result;
use crate::util::js::now_ms;

/// The last indexed mtime (ms, possibly fractional) of `path`.
pub fn get_sync_mtime(conn: &Connection, path: &str) -> Result<Option<f64>> {
    Ok(conn
        .query_row("SELECT mtime FROM sync WHERE path = ?", [path], |r| {
            r.get(0)
        })
        .optional()?)
}

/// Record that `path` was indexed at `mtime`. `indexed_at` defaults to now;
/// batch jobs can pin one value across many rows.
pub fn set_sync_mtime(
    conn: &Connection,
    path: &str,
    mtime: f64,
    indexed_at: Option<i64>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO sync (path, mtime, indexed_at) VALUES (?, ?, ?)
     ON CONFLICT(path) DO UPDATE SET
       mtime = excluded.mtime,
       indexed_at = excluded.indexed_at",
        params![path, mtime, indexed_at.unwrap_or_else(now_ms)],
    )?;
    Ok(())
}

/// Every tracked path. The pipeline diffs it against the disk to find
/// deletions.
pub fn get_all_sync_paths(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT path FROM sync")?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Remove the sync row of `path`.
pub fn delete_sync_path(conn: &Connection, path: &str) -> Result<()> {
    conn.execute("DELETE FROM sync WHERE path = ?", [path])?;
    Ok(())
}

/// The last indexed mtime (ms) of every tracked path.
pub fn all_sync_mtimes(conn: &Connection) -> Result<HashMap<String, f64>> {
    let mut stmt = conn.prepare("SELECT path, mtime FROM sync")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::super::db::open_db;
    use super::*;

    #[test]
    fn tracks_sync_state() {
        let db = open_db(":memory:").unwrap();
        set_sync_mtime(&db, "test.md", 1000.0, None).unwrap();
        assert_eq!(get_sync_mtime(&db, "test.md").unwrap(), Some(1000.0));
        set_sync_mtime(&db, "test.md", 2000.0, None).unwrap();
        assert_eq!(get_sync_mtime(&db, "test.md").unwrap(), Some(2000.0));
    }

    #[test]
    fn keeps_fractional_mtimes() {
        let db = open_db(":memory:").unwrap();
        set_sync_mtime(&db, "a.md", 1700000000123.456, Some(5)).unwrap();
        assert_eq!(
            get_sync_mtime(&db, "a.md").unwrap(),
            Some(1700000000123.456)
        );
        assert_eq!(all_sync_mtimes(&db).unwrap()["a.md"], 1700000000123.456);
    }

    #[test]
    fn unknown_path_is_none() {
        let db = open_db(":memory:").unwrap();
        assert_eq!(get_sync_mtime(&db, "missing.md").unwrap(), None);
    }

    #[test]
    fn lists_all_sync_paths() {
        let db = open_db(":memory:").unwrap();
        set_sync_mtime(&db, "a.md", 1.0, None).unwrap();
        set_sync_mtime(&db, "b.md", 2.0, None).unwrap();
        let mut paths = get_all_sync_paths(&db).unwrap();
        paths.sort();
        assert_eq!(paths, ["a.md", "b.md"]);
    }

    #[test]
    fn delete_removes_the_entry() {
        let db = open_db(":memory:").unwrap();
        set_sync_mtime(&db, "a.md", 1.0, None).unwrap();
        delete_sync_path(&db, "a.md").unwrap();
        assert_eq!(get_sync_mtime(&db, "a.md").unwrap(), None);
        assert!(get_all_sync_paths(&db).unwrap().is_empty());
    }
}
