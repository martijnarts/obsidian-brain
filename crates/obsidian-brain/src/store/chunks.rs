//! Chunk rows and their vectors in `chunks_vec`, keyed by the chunk rowid.

use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

use super::{Result, vector_blob};
use crate::types::Chunk;

/// The composite chunk id, `<node id>::<chunk index>`.
pub fn chunk_id(node_id: &str, chunk_index: usize) -> String {
    format!("{node_id}::{chunk_index}")
}

/// A row of the chunks table. Its vector lives in `chunks_vec` at `rowid`.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkRow {
    pub id: String,
    pub node_id: String,
    pub chunk_index: i64,
    pub heading: Option<String>,
    pub heading_level: Option<i64>,
    pub content: String,
    pub content_hash: String,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
    pub rowid: i64,
}

/// One kNN hit over `chunks_vec`, joined with its chunk and note title.
/// `score` is `1 - distance`.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkSearchHit {
    pub chunk_id: String,
    pub node_id: String,
    pub chunk_index: i64,
    pub heading: Option<String>,
    pub heading_level: Option<i64>,
    pub content: String,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
    pub score: f64,
    pub title: String,
}

/// Upsert one chunk row and return its rowid, for [`upsert_chunk_vector`].
pub fn upsert_chunk_row(conn: &Connection, node_id: &str, chunk: &Chunk) -> Result<i64> {
    let id = chunk_id(node_id, chunk.chunk_index);
    conn.execute(
        "INSERT INTO chunks
       (id, node_id, chunk_index, heading, heading_level, content, content_hash, start_line, end_line)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
     ON CONFLICT(id) DO UPDATE SET
       node_id        = excluded.node_id,
       chunk_index    = excluded.chunk_index,
       heading        = excluded.heading,
       heading_level  = excluded.heading_level,
       content        = excluded.content,
       content_hash   = excluded.content_hash,
       start_line     = excluded.start_line,
       end_line       = excluded.end_line",
        params![
            id,
            node_id,
            chunk.chunk_index as i64,
            chunk.heading,
            chunk.heading_level,
            chunk.content,
            chunk.content_hash,
            chunk.start_line as i64,
            chunk.end_line as i64
        ],
    )?;
    Ok(conn.query_row("SELECT rowid FROM chunks WHERE id = ?", [&id], |r| r.get(0))?)
}

/// Write `vector` into `chunks_vec` at `rowid`, replacing any old row.
pub fn upsert_chunk_vector(conn: &Connection, rowid: i64, vector: &[f32]) -> Result<()> {
    conn.execute("DELETE FROM chunks_vec WHERE rowid = ?", [rowid])?;
    conn.execute(
        "INSERT INTO chunks_vec(rowid, embedding) VALUES (?, ?)",
        params![rowid, vector_blob(vector)],
    )?;
    Ok(())
}

/// A chunk by composite id. The indexer uses it to skip re-embedding a
/// chunk whose `content_hash` did not change.
pub fn get_chunk(conn: &Connection, id: &str) -> Result<Option<ChunkRow>> {
    Ok(conn
        .query_row(
            "SELECT rowid, id, node_id, chunk_index, heading, heading_level,
              content, content_hash, start_line, end_line
         FROM chunks WHERE id = ?",
            [id],
            |r| {
                Ok(ChunkRow {
                    rowid: r.get(0)?,
                    id: r.get(1)?,
                    node_id: r.get(2)?,
                    chunk_index: r.get(3)?,
                    heading: r.get(4)?,
                    heading_level: r.get(5)?,
                    content: r.get(6)?,
                    content_hash: r.get(7)?,
                    start_line: r.get(8)?,
                    end_line: r.get(9)?,
                })
            },
        )
        .optional()?)
}

/// The ids of every chunk of `node_id`, in chunk order.
pub fn get_chunk_ids_for_node(conn: &Connection, node_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM chunks WHERE node_id = ? ORDER BY chunk_index")?;
    let rows = stmt.query_map([node_id], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Delete chunks and their vectors by composite id.
pub fn delete_chunks<S: AsRef<str>>(conn: &Connection, ids: &[S]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    let rowids: Vec<i64> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT rowid FROM chunks WHERE id IN ({placeholders})"
        ))?;
        let rows = stmt.query_map(params_from_iter(ids.iter().map(AsRef::as_ref)), |r| {
            r.get(0)
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for rowid in rowids {
        conn.execute("DELETE FROM chunks_vec WHERE rowid = ?", [rowid])?;
    }
    conn.execute(
        &format!("DELETE FROM chunks WHERE id IN ({placeholders})"),
        params_from_iter(ids.iter().map(AsRef::as_ref)),
    )?;
    Ok(())
}

/// Delete every chunk of `node_id` and its vector. `chunks_vec` has no
/// cascade, so the vectors go first.
pub fn delete_chunks_for_node(conn: &Connection, node_id: &str) -> Result<()> {
    let rowids: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT rowid FROM chunks WHERE node_id = ?")?;
        let rows = stmt.query_map([node_id], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for rowid in rowids {
        conn.execute("DELETE FROM chunks_vec WHERE rowid = ?", [rowid])?;
    }
    conn.execute("DELETE FROM chunks WHERE node_id = ?", [node_id])?;
    Ok(())
}

/// The total number of chunks.
pub fn count_chunks(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) AS n FROM chunks", [], |r| r.get(0))?)
}

/// kNN search over `chunks_vec`. One row per matching chunk, nearest
/// first; callers that want one row per note group by `node_id`.
pub fn search_chunk_vectors(
    conn: &Connection,
    vector: &[f32],
    limit: usize,
) -> Result<Vec<ChunkSearchHit>> {
    let mut stmt = conn.prepare(
        "SELECT v.rowid, v.distance,
              c.id AS chunk_id, c.node_id, c.chunk_index,
              c.heading, c.heading_level, c.content,
              c.start_line, c.end_line,
              n.title
         FROM chunks_vec v
         JOIN chunks c ON c.rowid = v.rowid
         JOIN nodes  n ON n.id    = c.node_id
        WHERE embedding MATCH ? AND k = ?
        ORDER BY distance",
    )?;
    let rows = stmt.query_map(params![vector_blob(vector), limit as i64], |r| {
        let distance: f64 = r.get(1)?;
        Ok(ChunkSearchHit {
            chunk_id: r.get(2)?,
            node_id: r.get(3)?,
            chunk_index: r.get(4)?,
            heading: r.get(5)?,
            heading_level: r.get(6)?,
            content: r.get(7)?,
            start_line: r.get(8)?,
            end_line: r.get(9)?,
            score: 1.0 - distance,
            title: r.get(10)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::super::db::open_db;
    use super::super::nodes::{delete_node, upsert_node};
    use super::*;
    use crate::types::ParsedNode;

    fn fake_chunk(i: usize, heading: &str, content: &str) -> Chunk {
        Chunk {
            chunk_index: i,
            heading: Some(heading.into()),
            heading_level: Some(1),
            content: content.into(),
            content_hash: format!("hash-{i}"),
            start_line: i + 1,
            end_line: i + 2,
        }
    }

    fn chunk(i: usize) -> Chunk {
        fake_chunk(i, "H", "body body")
    }

    pub(crate) fn mk_emb(seed: f64) -> Vec<f32> {
        let mut v: Vec<f32> = (0..384)
            .map(|i| (i as f64 * 0.1 + seed).sin() as f32)
            .collect();
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter_mut().for_each(|x| *x /= norm);
        v
    }

    fn setup() -> Connection {
        let db = open_db(":memory:").unwrap();
        for (id, title, content) in [("a.md", "A", "content A"), ("b.md", "B", "content B")] {
            upsert_node(
                &db,
                &ParsedNode {
                    id: id.into(),
                    title: title.into(),
                    content: content.into(),
                    frontmatter: Default::default(),
                },
            )
            .unwrap();
        }
        db
    }

    fn vec_count(db: &Connection, rowid: i64) -> i64 {
        db.query_row(
            "SELECT COUNT(*) AS n FROM chunks_vec WHERE rowid = ?",
            [rowid],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn upserts_a_chunk_row_and_retrieves_it() {
        let db = setup();
        let rowid = upsert_chunk_row(&db, "a.md", &chunk(0)).unwrap();
        assert!(rowid > 0);
        let c = get_chunk(&db, "a.md::0").unwrap().unwrap();
        assert_eq!(c.node_id, "a.md");
        assert_eq!(c.chunk_index, 0);
        assert_eq!(c.content_hash, "hash-0");
    }

    #[test]
    fn get_chunk_returns_none_for_missing_ids() {
        let db = setup();
        assert!(get_chunk(&db, "nope::0").unwrap().is_none());
    }

    #[test]
    fn upsert_replaces_on_conflict() {
        let db = setup();
        upsert_chunk_row(&db, "a.md", &fake_chunk(0, "Old", "old body")).unwrap();
        let mut c = fake_chunk(0, "New", "new body");
        c.content_hash = "hash-new".into();
        upsert_chunk_row(&db, "a.md", &c).unwrap();
        let got = get_chunk(&db, "a.md::0").unwrap().unwrap();
        assert_eq!(got.heading.as_deref(), Some("New"));
        assert_eq!(got.content_hash, "hash-new");
    }

    #[test]
    fn chunk_ids_for_node_are_sequential() {
        let db = setup();
        for i in 0..3 {
            upsert_chunk_row(&db, "a.md", &chunk(i)).unwrap();
        }
        upsert_chunk_row(&db, "b.md", &chunk(0)).unwrap();
        assert_eq!(
            get_chunk_ids_for_node(&db, "a.md").unwrap(),
            ["a.md::0", "a.md::1", "a.md::2"]
        );
        assert_eq!(get_chunk_ids_for_node(&db, "b.md").unwrap(), ["b.md::0"]);
    }

    #[test]
    fn delete_chunks_removes_rows_and_vectors() {
        let db = setup();
        let rid = upsert_chunk_row(&db, "a.md", &chunk(0)).unwrap();
        upsert_chunk_vector(&db, rid, &mk_emb(0.0)).unwrap();
        upsert_chunk_row(&db, "a.md", &chunk(1)).unwrap();
        delete_chunks(&db, &["a.md::0"]).unwrap();
        assert!(get_chunk(&db, "a.md::0").unwrap().is_none());
        assert!(get_chunk(&db, "a.md::1").unwrap().is_some());
        assert_eq!(vec_count(&db, rid), 0);
    }

    #[test]
    fn delete_chunks_for_node_wipes_one_node() {
        let db = setup();
        let r0 = upsert_chunk_row(&db, "a.md", &chunk(0)).unwrap();
        let r1 = upsert_chunk_row(&db, "a.md", &chunk(1)).unwrap();
        upsert_chunk_vector(&db, r0, &mk_emb(0.0)).unwrap();
        upsert_chunk_vector(&db, r1, &mk_emb(1.0)).unwrap();
        upsert_chunk_row(&db, "b.md", &chunk(0)).unwrap();
        delete_chunks_for_node(&db, "a.md").unwrap();
        assert!(get_chunk_ids_for_node(&db, "a.md").unwrap().is_empty());
        assert_eq!(get_chunk_ids_for_node(&db, "b.md").unwrap(), ["b.md::0"]);
        assert_eq!(vec_count(&db, r0), 0);
    }

    #[test]
    fn count_chunks_reports_total_rows() {
        let db = setup();
        assert_eq!(count_chunks(&db).unwrap(), 0);
        upsert_chunk_row(&db, "a.md", &chunk(0)).unwrap();
        upsert_chunk_row(&db, "a.md", &chunk(1)).unwrap();
        assert_eq!(count_chunks(&db).unwrap(), 2);
    }

    #[test]
    fn delete_node_cascades_to_chunks_and_vectors() {
        let db = setup();
        let rid = upsert_chunk_row(&db, "a.md", &chunk(0)).unwrap();
        upsert_chunk_vector(&db, rid, &mk_emb(0.0)).unwrap();
        delete_node(&db, "a.md").unwrap();
        assert!(get_chunk_ids_for_node(&db, "a.md").unwrap().is_empty());
        assert_eq!(vec_count(&db, rid), 0);
    }

    #[test]
    fn search_finds_the_nearest_chunk() {
        let db = setup();
        let v0 = mk_emb(0.0);
        let r0 = upsert_chunk_row(&db, "a.md", &chunk(0)).unwrap();
        let r1 = upsert_chunk_row(&db, "b.md", &chunk(0)).unwrap();
        upsert_chunk_vector(&db, r0, &v0).unwrap();
        upsert_chunk_vector(&db, r1, &mk_emb(5.0)).unwrap();
        let hits = search_chunk_vectors(&db, &v0, 5).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].node_id, "a.md");
        assert_eq!(hits[0].title, "A");
        assert!(hits[0].score > 0.99);
    }

    #[test]
    fn chunk_rows_need_an_existing_node() {
        let db = setup();
        assert!(upsert_chunk_row(&db, "missing.md", &chunk(0)).is_err());
    }
}
