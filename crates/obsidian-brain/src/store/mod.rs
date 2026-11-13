//! The SQLite index: schema and per-table queries.
//!
//! Every store function takes a `&rusqlite::Connection`, so callers can
//! run several of them inside one transaction. [`Db`] is the shared handle
//! that owns the connection.

pub mod chunks;
pub mod db;
pub mod edges;
pub mod embeddings;
pub mod fts5_escape;
pub mod fulltext;
pub mod metadata;
pub mod nodes;
pub mod rename;
pub mod sync;

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{Connection, Transaction};

pub use rusqlite;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Schema(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// The index of one vault: a single SQLite connection behind a mutex.
///
/// SQLite runs one writer at a time anyway, and one connection keeps the
/// FTS5 and vec0 tables consistent without cross-connection locking. WAL
/// mode stays on so a crash never corrupts the file. Tool handlers and the
/// background indexer share this handle. Hold the lock only for short
/// synchronous work and never across an `.await`; the indexer should write
/// in batches and release the lock between them so reads interleave.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    /// Open (or create) the index at `path`. See [`db::open_db`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self::from_connection(db::open_db(path)?))
    }

    /// A fresh in-memory index, for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::open(":memory:")
    }

    pub fn from_connection(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    /// Lock the connection. A panic in another holder does not poison the
    /// handle: SQLite rolls back any open statement on its own.
    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Run `f` with the connection locked.
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        f(&self.lock())
    }

    /// Run `f` inside one transaction. It commits when `f` returns `Ok`
    /// and rolls back when `f` returns `Err`.
    pub fn with_tx<T, E>(
        &self,
        f: impl FnOnce(&Transaction<'_>) -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E>
    where
        E: From<rusqlite::Error>,
    {
        let mut guard = self.lock();
        let tx = guard.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// Run `f` on a blocking thread with the connection locked, so an
    /// async handler does not stall the runtime on a long query.
    pub async fn run<T, F>(&self, f: F) -> T
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> T + Send + 'static,
    {
        let db = self.clone();
        match tokio::task::spawn_blocking(move || db.with(f)).await {
            Ok(v) => v,
            Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
            Err(e) => panic!("store task cancelled: {e}"),
        }
    }
}

/// Run `f` inside a SAVEPOINT, so it is atomic both on its own and inside
/// a caller's transaction.
pub(crate) fn savepoint<T>(
    conn: &Connection,
    name: &str,
    f: impl FnOnce() -> Result<T>,
) -> Result<T> {
    conn.execute_batch(&format!("SAVEPOINT {name}"))?;
    match f() {
        Ok(v) => {
            conn.execute_batch(&format!("RELEASE {name}"))?;
            Ok(v)
        }
        Err(e) => {
            // Roll back, then pop the savepoint off the stack.
            let _ = conn.execute_batch(&format!("ROLLBACK TO {name}; RELEASE {name}"));
            Err(e)
        }
    }
}

/// Encode a vector as the little-endian float32 blob that vec0 reads.
pub(crate) fn vector_blob(vector: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vector.len() * 4);
    for v in vector {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_executes_on_a_blocking_thread() {
        let db = Db::open_in_memory().unwrap();
        let n: i64 = db
            .run(|c| {
                c.query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0))
                    .unwrap()
            })
            .await;
        assert_eq!(n, 0);
    }

    #[test]
    fn with_tx_rolls_back_on_error() {
        let db = Db::open_in_memory().unwrap();
        let res: Result<()> = db.with_tx(|tx| {
            metadata::set_metadata(tx, "k", "v")?;
            Err(StoreError::Schema("stop".into()))
        });
        assert!(res.is_err());
        assert_eq!(db.with(|c| metadata::get_metadata(c, "k").unwrap()), None);
    }
}
