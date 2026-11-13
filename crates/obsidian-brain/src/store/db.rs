//! Opening the index, its schema, and the embedding-table helpers.

use std::path::Path;
use std::sync::Once;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};

use super::{Result, StoreError};

/// The vec0 dim used before an embedder declares its own. Matches the
/// default 384-dim models. [`ensure_vec_tables`] reconciles it at runtime.
pub const DEFAULT_EMBEDDING_DIM: usize = 384;

/// The layout version of the index, kept under `schema_version` in
/// `index_metadata`. On a mismatch the index is dropped and rebuilt, so
/// there are no in-place migrations.
pub const SCHEMA_VERSION: i64 = 1;

/// The FTS5 tokenize clause of `nodes_fts`.
pub const EXPECTED_FTS_TOKENIZE: &str = "porter unicode61";

static VEC_INIT: Once = Once::new();

/// Register sqlite-vec for every connection opened after this call.
pub fn register_sqlite_vec() {
    VEC_INIT.call_once(|| {
        // SAFETY: sqlite3_vec_init has the signature of a SQLite extension
        // entry point; sqlite3_auto_extension stores the pointer and calls
        // it for each new connection.
        unsafe {
            #[allow(clippy::missing_transmute_annotations)]
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
    });
}

/// Open the SQLite database at `path` (`:memory:` works), enable WAL and
/// foreign keys, load sqlite-vec and create the schema. A database written
/// with another [`SCHEMA_VERSION`] is emptied and rebuilt.
pub fn open_db(path: impl AsRef<Path>) -> Result<Connection> {
    register_sqlite_vec();
    let conn = Connection::open(path)?;
    conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get::<_, String>(0))?;
    conn.busy_timeout(Duration::from_millis(5000))?;
    // The chunks cascade relies on foreign keys.
    conn.execute_batch("PRAGMA foreign_keys = ON")?;

    match stored_schema_version(&conn)? {
        Some(v) if v == SCHEMA_VERSION => init_schema(&conn)?,
        None if !has_user_tables(&conn)? => {
            init_schema(&conn)?;
            write_schema_version(&conn)?;
        }
        stored => {
            tracing::warn!(
                stored = ?stored,
                expected = SCHEMA_VERSION,
                "index schema version mismatch, rebuilding the index"
            );
            drop_all_tables(&conn)?;
            init_schema(&conn)?;
            write_schema_version(&conn)?;
        }
    }
    self_check_schema(&conn)?;
    Ok(conn)
}

fn stored_schema_version(conn: &Connection) -> Result<Option<i64>> {
    if !table_exists(conn, "index_metadata")? {
        return Ok(None);
    }
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM index_metadata WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.and_then(|s| s.parse().ok()))
}

fn write_schema_version(conn: &Connection) -> Result<()> {
    super::metadata::set_metadata(conn, "schema_version", &SCHEMA_VERSION.to_string())
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
        [name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn has_user_tables(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Drop every table. Virtual tables go first, so their shadow tables go
/// with them.
fn drop_all_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA foreign_keys = OFF")?;
    let list = |sql: &str| -> Result<Vec<String>> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    };
    for name in list(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND sql LIKE 'CREATE VIRTUAL TABLE%'",
    )? {
        conn.execute_batch(&format!(
            "DROP TABLE IF EXISTS \"{}\"",
            name.replace('"', "\"\"")
        ))?;
    }
    for name in
        list("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")?
    {
        conn.execute_batch(&format!(
            "DROP TABLE IF EXISTS \"{}\"",
            name.replace('"', "\"\"")
        ))?;
    }
    conn.execute_batch("PRAGMA foreign_keys = ON")?;
    Ok(())
}

/// Create every table, index and virtual table (FTS5 and vec0). Idempotent.
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(&format!(
        "
    CREATE TABLE IF NOT EXISTS nodes (
      id TEXT PRIMARY KEY,
      title TEXT NOT NULL,
      content TEXT,
      frontmatter TEXT NOT NULL DEFAULT '{{}}'
    );

    CREATE TABLE IF NOT EXISTS edges (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      source_id TEXT NOT NULL,
      target_id TEXT NOT NULL,
      context TEXT NOT NULL DEFAULT '',
      target_subpath TEXT
    );

    CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_id);
    CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_id);

    CREATE TABLE IF NOT EXISTS sync (
      path TEXT PRIMARY KEY,
      mtime INTEGER NOT NULL,
      indexed_at INTEGER NOT NULL
    );

    CREATE TABLE IF NOT EXISTS index_metadata (
      key TEXT PRIMARY KEY,
      value TEXT NOT NULL,
      updated_at INTEGER NOT NULL
    );

    CREATE TABLE IF NOT EXISTS chunks (
      id TEXT PRIMARY KEY,
      node_id TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
      chunk_index INTEGER NOT NULL,
      heading TEXT,
      heading_level INTEGER,
      content TEXT NOT NULL,
      content_hash TEXT NOT NULL,
      start_line INTEGER,
      end_line INTEGER
    );

    CREATE INDEX IF NOT EXISTS idx_chunks_node_id ON chunks(node_id);
    CREATE INDEX IF NOT EXISTS idx_chunks_hash ON chunks(content_hash);

    CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts
      USING fts5(title, content, content='nodes', content_rowid='rowid', tokenize='{EXPECTED_FTS_TOKENIZE}');

    CREATE VIRTUAL TABLE IF NOT EXISTS nodes_vec
      USING vec0(embedding float[{DEFAULT_EMBEDDING_DIM}]);

    CREATE VIRTUAL TABLE IF NOT EXISTS chunks_vec
      USING vec0(embedding float[{DEFAULT_EMBEDDING_DIM}]);
    "
    ))?;
    create_embedder_capability_table(conn)?;
    create_failed_chunks_table(conn)?;
    Ok(())
}

const EMBEDDER_CAPABILITY_SQL: &str = "
    CREATE TABLE IF NOT EXISTS embedder_capability (
      embedder_id TEXT NOT NULL,
      model_hash TEXT NOT NULL,
      advertised_max_tokens INTEGER,
      discovered_max_tokens INTEGER,
      discovered_at INTEGER,
      method TEXT,
      dim INTEGER,
      query_prefix TEXT,
      document_prefix TEXT,
      prefix_source TEXT,
      base_model TEXT,
      size_bytes INTEGER,
      fetched_at INTEGER,
      PRIMARY KEY (embedder_id, model_hash)
    );
";

const FAILED_CHUNKS_SQL: &str = "
    CREATE TABLE IF NOT EXISTS failed_chunks (
      chunk_id TEXT PRIMARY KEY,
      note_id TEXT NOT NULL,
      reason TEXT NOT NULL,
      error_message TEXT,
      failed_at INTEGER NOT NULL
    );
";

/// Create `embedder_capability`: per-model context-length probes and the
/// cached model metadata (dim, prefixes, base model, size).
pub fn create_embedder_capability_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(EMBEDDER_CAPABILITY_SQL)?;
    Ok(())
}

/// Create `failed_chunks`: chunks the indexer could not embed.
pub fn create_failed_chunks_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(FAILED_CHUNKS_SQL)?;
    Ok(())
}

const EMBEDDER_CAPABILITY_COLUMNS: &[&str] = &[
    "embedder_id",
    "model_hash",
    "advertised_max_tokens",
    "discovered_max_tokens",
    "discovered_at",
    "method",
    "dim",
    "query_prefix",
    "document_prefix",
    "prefix_source",
    "base_model",
    "size_bytes",
    "fetched_at",
];

const FAILED_CHUNKS_COLUMNS: &[&str] = &[
    "chunk_id",
    "note_id",
    "reason",
    "error_message",
    "failed_at",
];

fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |r| r.get::<_, String>("name"))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Cross-check the live columns of `embedder_capability` and
/// `failed_chunks` against the columns the code reads and writes. Both
/// hold caches and telemetry only, so a missing table is recreated and a
/// table with missing columns is dropped and recreated. Extra columns are
/// logged and left alone. Returns the messages it logged.
pub fn self_check_schema(conn: &Connection) -> Result<Vec<String>> {
    let mut messages = Vec::new();
    let expected: [(&str, &[&str], &str); 2] = [
        (
            "embedder_capability",
            EMBEDDER_CAPABILITY_COLUMNS,
            EMBEDDER_CAPABILITY_SQL,
        ),
        ("failed_chunks", FAILED_CHUNKS_COLUMNS, FAILED_CHUNKS_SQL),
    ];
    for (table, want, create) in expected {
        let cols = table_columns(conn, table)?;
        if cols.is_empty() {
            let msg = format!("schema-check: {table} missing — recreating");
            tracing::warn!(table, "{msg}");
            messages.push(msg);
            conn.execute_batch(create)?;
            continue;
        }
        let missing: Vec<&str> = want
            .iter()
            .copied()
            .filter(|c| !cols.iter().any(|x| x == c))
            .collect();
        if !missing.is_empty() {
            let msg = format!(
                "schema-check: {table} is missing columns [{}] — dropping and recreating it",
                missing.join(", ")
            );
            tracing::warn!(table, "{msg}");
            messages.push(msg);
            conn.execute_batch(&format!("DROP TABLE {table}"))?;
            conn.execute_batch(create)?;
            continue;
        }
        let extra: Vec<&str> = cols
            .iter()
            .map(String::as_str)
            .filter(|c| !want.contains(c))
            .collect();
        if !extra.is_empty() {
            let msg = format!(
                "schema-check: {table} has unexpected columns [{}] (forward-compat from a newer version)",
                extra.join(", ")
            );
            tracing::info!(table, "{msg}");
            messages.push(msg);
        }
    }
    Ok(messages)
}

fn table_sql(conn: &Connection, table: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?",
            [table],
            |r| r.get::<_, String>(0),
        )
        .optional()?)
}

/// The column dim of a vec0 table, read from its stored CREATE statement.
fn read_vec_dim(conn: &Connection, table: &str) -> Result<Option<usize>> {
    let Some(sql) = table_sql(conn, table)? else {
        return Ok(None);
    };
    let re = regex::Regex::new(r"float\[(\d+)\]").expect("valid regex");
    Ok(re.captures(&sql).and_then(|c| c[1].parse().ok()))
}

/// The tokenize clause of an FTS5 table, read from its stored CREATE
/// statement.
fn read_fts_tokenize(conn: &Connection, table: &str) -> Result<Option<String>> {
    let Some(sql) = table_sql(conn, table)? else {
        return Ok(None);
    };
    let re = regex::Regex::new(r#"(?i)tokenize\s*=\s*['"]([^'"]+)['"]"#).expect("valid regex");
    Ok(re.captures(&sql).map(|c| c[1].to_string()))
}

/// Reconcile the dims of `nodes_vec` and `chunks_vec` with the embedder's
/// output dim. An empty table at the wrong dim is recreated; a table with
/// rows at the wrong dim is an error that asks for a rebuild.
pub fn ensure_vec_tables(conn: &Connection, dim: usize) -> Result<()> {
    ensure_single_vec_table(conn, "nodes_vec", dim)?;
    ensure_single_vec_table(conn, "chunks_vec", dim)
}

fn ensure_single_vec_table(conn: &Connection, table: &str, dim: usize) -> Result<()> {
    let current = read_vec_dim(conn, table)?;
    if current == Some(dim) {
        return Ok(());
    }
    if let Some(current) = current {
        let rows: i64 = conn.query_row(&format!("SELECT COUNT(*) AS n FROM {table}"), [], |r| {
            r.get(0)
        })?;
        if rows > 0 {
            return Err(StoreError::Schema(format!(
                "Embedding dimension mismatch: existing {table} has dim {current}, \
                 current model produces dim {dim}. \
                 Re-index from scratch: run `obsidian-brain index --drop`."
            )));
        }
        conn.execute_batch(&format!("DROP TABLE {table}"))?;
    }
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE {table} USING vec0(embedding float[{dim}])"
    ))?;
    Ok(())
}

/// Drop every embedding, chunk, sync row, capability probe and failed
/// chunk, so the next index run rebuilds from scratch. Run
/// [`ensure_vec_tables`] afterwards to recreate the vec0 tables.
pub fn drop_embedding_state(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS nodes_vec;
         DROP TABLE IF EXISTS chunks_vec;
         DELETE FROM chunks;
         DELETE FROM sync;
         DELETE FROM embedder_capability;
         DELETE FROM failed_chunks;",
    )?;
    Ok(())
}

/// Drop `nodes_fts`, recreate it with the current tokenize clause and
/// re-ingest every node.
pub fn rebuild_full_text_index(conn: &Connection) -> Result<()> {
    conn.execute_batch(&format!(
        "DROP TABLE IF EXISTS nodes_fts;
         CREATE VIRTUAL TABLE nodes_fts USING fts5(title, content, content='nodes', content_rowid='rowid', tokenize='{EXPECTED_FTS_TOKENIZE}');"
    ))?;
    super::savepoint(conn, "rebuild_fts", || {
        let mut select = conn.prepare("SELECT rowid, title, content FROM nodes")?;
        let mut insert =
            conn.prepare("INSERT INTO nodes_fts(rowid, title, content) VALUES(?, ?, ?)")?;
        let mut rows = select.query([])?;
        while let Some(r) = rows.next()? {
            let rowid: i64 = r.get(0)?;
            let title: String = r.get(1)?;
            let content: Option<String> = r.get(2)?;
            insert.execute(params![rowid, title, content.unwrap_or_default()])?;
        }
        Ok(())
    })
}

/// The stored tokenize clause of `nodes_fts`.
pub fn current_fts_tokenize(conn: &Connection) -> Result<Option<String>> {
    read_fts_tokenize(conn, "nodes_fts")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    #[test]
    fn creates_schema_on_initialization() {
        let conn = open_db(":memory:").unwrap();
        let t = tables(&conn);
        for name in [
            "nodes",
            "edges",
            "sync",
            "index_metadata",
            "chunks",
            "nodes_fts",
            "nodes_vec",
            "chunks_vec",
            "embedder_capability",
            "failed_chunks",
        ] {
            assert!(t.contains(&name.to_string()), "missing {name}");
        }
        assert_eq!(
            super::super::metadata::get_metadata(&conn, "schema_version").unwrap(),
            Some(SCHEMA_VERSION.to_string())
        );
        assert_eq!(
            current_fts_tokenize(&conn).unwrap().as_deref(),
            Some(EXPECTED_FTS_TOKENIZE)
        );
    }

    #[test]
    fn rebuilds_on_schema_version_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        {
            let conn = open_db(&path).unwrap();
            conn.execute("INSERT INTO nodes (id, title) VALUES ('a.md', 'A')", [])
                .unwrap();
            conn.execute_batch("CREATE TABLE communities (id INTEGER PRIMARY KEY)")
                .unwrap();
            super::super::metadata::set_metadata(&conn, "schema_version", "0").unwrap();
        }
        let conn = open_db(&path).unwrap();
        let t = tables(&conn);
        assert!(!t.contains(&"communities".to_string()));
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
        assert_eq!(
            super::super::metadata::get_metadata(&conn, "schema_version").unwrap(),
            Some(SCHEMA_VERSION.to_string())
        );
    }

    #[test]
    fn keeps_data_when_schema_version_matches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        {
            let conn = open_db(&path).unwrap();
            conn.execute("INSERT INTO nodes (id, title) VALUES ('a.md', 'A')", [])
                .unwrap();
        }
        let conn = open_db(&path).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn rebuilds_a_foreign_database_without_schema_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE stray (x TEXT)").unwrap();
        }
        let conn = open_db(&path).unwrap();
        assert!(!tables(&conn).contains(&"stray".to_string()));
    }

    #[test]
    fn self_check_heals_missing_capability_table() {
        let conn = open_db(":memory:").unwrap();
        conn.execute_batch("DROP TABLE embedder_capability")
            .unwrap();
        let msgs = self_check_schema(&conn).unwrap();
        assert!(
            msgs.iter()
                .any(|m| m.contains("embedder_capability") && m.contains("missing"))
        );
        let cols = table_columns(&conn, "embedder_capability").unwrap();
        for c in EMBEDDER_CAPABILITY_COLUMNS {
            assert!(cols.contains(&c.to_string()));
        }
    }

    #[test]
    fn self_check_recreates_table_with_missing_columns() {
        let conn = open_db(":memory:").unwrap();
        conn.execute_batch(
            "DROP TABLE embedder_capability;
             CREATE TABLE embedder_capability (embedder_id TEXT NOT NULL, model_hash TEXT NOT NULL, PRIMARY KEY (embedder_id, model_hash));",
        )
        .unwrap();
        let msgs = self_check_schema(&conn).unwrap();
        assert!(msgs.iter().any(|m| m.contains("missing columns")));
        assert_eq!(
            table_columns(&conn, "embedder_capability").unwrap().len(),
            EMBEDDER_CAPABILITY_COLUMNS.len()
        );
    }

    #[test]
    fn self_check_reports_extra_columns_without_failing() {
        let conn = open_db(":memory:").unwrap();
        conn.execute_batch("ALTER TABLE failed_chunks ADD COLUMN extra_col TEXT")
            .unwrap();
        let msgs = self_check_schema(&conn).unwrap();
        assert!(
            msgs.iter()
                .any(|m| m.contains("failed_chunks") && m.contains("extra_col"))
        );
        let cols = table_columns(&conn, "failed_chunks").unwrap();
        for c in FAILED_CHUNKS_COLUMNS {
            assert!(cols.contains(&c.to_string()));
        }
    }

    #[test]
    fn self_check_is_quiet_and_idempotent_on_a_clean_db() {
        let conn = open_db(":memory:").unwrap();
        assert!(self_check_schema(&conn).unwrap().is_empty());
        assert!(self_check_schema(&conn).unwrap().is_empty());
    }

    #[test]
    fn ensure_vec_tables_recreates_empty_tables_and_refuses_full_ones() {
        let conn = open_db(":memory:").unwrap();
        ensure_vec_tables(&conn, 768).unwrap();
        assert_eq!(read_vec_dim(&conn, "nodes_vec").unwrap(), Some(768));
        assert_eq!(read_vec_dim(&conn, "chunks_vec").unwrap(), Some(768));
        let v = vec![0.5f32; 768];
        conn.execute(
            "INSERT INTO nodes_vec(rowid, embedding) VALUES (1, ?)",
            [super::super::vector_blob(&v)],
        )
        .unwrap();
        let err = ensure_vec_tables(&conn, 384).unwrap_err().to_string();
        assert!(err.contains("Embedding dimension mismatch: existing nodes_vec has dim 768"));
    }

    #[test]
    fn drop_embedding_state_clears_tables() {
        let conn = open_db(":memory:").unwrap();
        super::super::sync::set_sync_mtime(&conn, "a.md", 1.0, None).unwrap();
        drop_embedding_state(&conn).unwrap();
        assert_eq!(read_vec_dim(&conn, "nodes_vec").unwrap(), None);
        assert!(
            super::super::sync::get_all_sync_paths(&conn)
                .unwrap()
                .is_empty()
        );
        ensure_vec_tables(&conn, 384).unwrap();
        assert_eq!(read_vec_dim(&conn, "nodes_vec").unwrap(), Some(384));
    }

    #[test]
    fn rebuild_full_text_index_reingests_nodes() {
        let conn = open_db(":memory:").unwrap();
        super::super::nodes::upsert_node(
            &conn,
            &crate::types::ParsedNode {
                id: "a.md".into(),
                title: "Alpha".into(),
                content: "zebra crossing".into(),
                frontmatter: Default::default(),
            },
        )
        .unwrap();
        rebuild_full_text_index(&conn).unwrap();
        let hits = super::super::fulltext::search_full_text(&conn, "zebra", 20).unwrap();
        assert_eq!(hits.len(), 1);
    }
}
