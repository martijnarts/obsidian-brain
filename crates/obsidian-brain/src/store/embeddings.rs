//! Note-level embeddings in `nodes_vec`, keyed by the node rowid.

use rusqlite::{Connection, params};

use super::nodes::get_node;
use super::{Result, js_trim, utf16_len, utf16_prefix, vector_blob};
use crate::types::SearchResult;

/// Insert or replace the embedding of `node_id`. A no-op when the node is
/// missing.
pub fn upsert_embedding(conn: &Connection, node_id: &str, vector: &[f32]) -> Result<()> {
    let Some(node) = get_node(conn, node_id)? else {
        return Ok(());
    };
    conn.execute("DELETE FROM nodes_vec WHERE rowid = ?", [node.rowid])?;
    conn.execute(
        "INSERT INTO nodes_vec(rowid, embedding) VALUES (?, ?)",
        params![node.rowid, vector_blob(vector)],
    )?;
    Ok(())
}

/// kNN search over `nodes_vec`. The score is `1 - distance` (higher is
/// better); the excerpt is the first paragraph, cut at 200 characters.
pub fn search_vector(conn: &Connection, vector: &[f32], limit: usize) -> Result<Vec<SearchResult>> {
    let mut stmt = conn.prepare(
        "SELECT v.rowid, v.distance, n.id, n.title, n.content
       FROM nodes_vec v
       JOIN nodes n ON n.rowid = v.rowid
       WHERE embedding MATCH ? AND k = ?
       ORDER BY distance",
    )?;
    let rows = stmt.query_map(params![vector_blob(vector), limit as i64], |r| {
        let distance: f64 = r.get(1)?;
        let content: Option<String> = r.get(4)?;
        Ok(SearchResult {
            node_id: r.get(2)?,
            title: r.get(3)?,
            score: 1.0 - distance,
            excerpt: first_paragraph(content.as_deref().unwrap_or(""), 200),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Remove the embedding of `node_id`, if any.
pub fn delete_embedding(conn: &Connection, node_id: &str) -> Result<()> {
    let Some(node) = get_node(conn, node_id)? else {
        return Ok(());
    };
    conn.execute("DELETE FROM nodes_vec WHERE rowid = ?", [node.rowid])?;
    Ok(())
}

/// The first non-empty paragraph that is not a heading, trimmed and cut to
/// `max_len` UTF-16 units with `...` appended.
fn first_paragraph(content: &str, max_len: usize) -> String {
    let re = regex::Regex::new(r"\n\n+").expect("valid regex");
    let Some(para) = re
        .split(content)
        .find(|p| !js_trim(p).is_empty() && !p.starts_with('#'))
    else {
        return String::new();
    };
    let trimmed = js_trim(para);
    if utf16_len(trimmed) > max_len {
        format!("{}...", utf16_prefix(trimmed, max_len))
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::super::db::open_db;
    use super::super::nodes::{delete_node, upsert_node};
    use super::*;
    use crate::types::ParsedNode;

    fn emb(seed: f64) -> Vec<f32> {
        let mut v: Vec<f32> = (0..384)
            .map(|i| ((i as f64 * 0.17 + seed).sin() * 0.9) as f32)
            .collect();
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter_mut().for_each(|x| *x /= norm);
        v
    }

    fn setup() -> Connection {
        let db = open_db(":memory:").unwrap();
        for (id, title, content) in [
            ("a.md", "Alpha", "alpha body"),
            ("b.md", "Beta", "beta body"),
        ] {
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

    #[test]
    fn upserts_and_searches_embeddings() {
        let db = setup();
        let va = emb(0.0);
        upsert_embedding(&db, "a.md", &va).unwrap();
        upsert_embedding(&db, "b.md", &emb(1.0)).unwrap();
        let results = search_vector(&db, &va, 5).unwrap();
        assert_eq!(results[0].node_id, "a.md");
        assert!(results[0].score > 0.99);
        assert_eq!(results[0].excerpt, "alpha body");
    }

    #[test]
    fn upsert_replaces_the_existing_row() {
        let db = setup();
        let v2 = emb(5.0);
        upsert_embedding(&db, "a.md", &emb(0.0)).unwrap();
        upsert_embedding(&db, "a.md", &v2).unwrap();
        let results = search_vector(&db, &v2, 5).unwrap();
        assert_eq!(results[0].node_id, "a.md");
        assert!(results[0].score > 0.99);
    }

    #[test]
    fn upsert_is_a_noop_for_a_missing_node() {
        let db = setup();
        upsert_embedding(&db, "nonexistent.md", &emb(3.0)).unwrap();
    }

    #[test]
    fn delete_embedding_removes_the_vector() {
        let db = setup();
        let v = emb(0.0);
        upsert_embedding(&db, "a.md", &v).unwrap();
        delete_embedding(&db, "a.md").unwrap();
        assert!(
            search_vector(&db, &v, 5)
                .unwrap()
                .iter()
                .all(|r| r.node_id != "a.md")
        );
    }

    #[test]
    fn delete_node_cascades_to_embedding() {
        let db = setup();
        let v = emb(0.0);
        upsert_embedding(&db, "a.md", &v).unwrap();
        delete_node(&db, "a.md").unwrap();
        assert!(
            search_vector(&db, &v, 5)
                .unwrap()
                .iter()
                .all(|r| r.node_id != "a.md")
        );
    }

    #[test]
    fn first_paragraph_skips_headings_and_cuts() {
        assert_eq!(
            first_paragraph("# Title\n\n  body text  \n\nmore", 200),
            "body text"
        );
        assert_eq!(first_paragraph("# only", 200), "");
        assert_eq!(first_paragraph("abcdef", 3), "abc...");
    }
}
