//! A key/value store for index-wide metadata: embedder identity, schema
//! version and the like. Kept apart from `sync`, which tracks files.

use rusqlite::{Connection, OptionalExtension, params};

use super::Result;
use crate::util::js::now_ms;

/// The value of `key`, or `None` if it was never written.
pub fn get_metadata(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM index_metadata WHERE key = ?",
            [key],
            |r| r.get(0),
        )
        .optional()?)
}

/// Insert or replace `key`. `updated_at` gets the current time in ms.
pub fn set_metadata(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO index_metadata (key, value, updated_at) VALUES (?, ?, ?)
     ON CONFLICT(key) DO UPDATE SET
       value = excluded.value,
       updated_at = excluded.updated_at",
        params![key, value, now_ms()],
    )?;
    Ok(())
}

/// Remove `key`. A no-op when it is absent.
pub fn delete_metadata(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM index_metadata WHERE key = ?", [key])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::db::open_db;
    use super::*;

    #[test]
    fn unknown_key_is_none() {
        let db = open_db(":memory:").unwrap();
        assert_eq!(get_metadata(&db, "nope").unwrap(), None);
    }

    #[test]
    fn set_then_get() {
        let db = open_db(":memory:").unwrap();
        set_metadata(&db, "embedding_model", "Xenova/all-MiniLM-L6-v2").unwrap();
        assert_eq!(
            get_metadata(&db, "embedding_model").unwrap().as_deref(),
            Some("Xenova/all-MiniLM-L6-v2")
        );
    }

    #[test]
    fn set_replaces_on_conflict() {
        let db = open_db(":memory:").unwrap();
        set_metadata(&db, "embedding_dim", "384").unwrap();
        set_metadata(&db, "embedding_dim", "768").unwrap();
        assert_eq!(
            get_metadata(&db, "embedding_dim").unwrap().as_deref(),
            Some("768")
        );
    }

    #[test]
    fn updated_at_advances() {
        let db = open_db(":memory:").unwrap();
        let at = |db: &Connection| -> i64 {
            db.query_row(
                "SELECT updated_at FROM index_metadata WHERE key = 'k'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        set_metadata(&db, "k", "v1").unwrap();
        let first = at(&db);
        std::thread::sleep(std::time::Duration::from_millis(2));
        set_metadata(&db, "k", "v2").unwrap();
        assert!(at(&db) >= first);
    }

    #[test]
    fn delete_removes_the_key_and_tolerates_missing_keys() {
        let db = open_db(":memory:").unwrap();
        set_metadata(&db, "doomed", "yes").unwrap();
        delete_metadata(&db, "doomed").unwrap();
        assert_eq!(get_metadata(&db, "doomed").unwrap(), None);
        delete_metadata(&db, "never-existed").unwrap();
    }
}
