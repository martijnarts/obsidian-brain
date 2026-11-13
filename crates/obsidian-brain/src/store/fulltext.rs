//! FTS5 search over note titles and content.

use rusqlite::{Connection, params};

use super::Result;
use super::fts5_escape::escape_fts5_query;
use crate::types::SearchResult;

/// FTS5 search across titles and content.
///
/// bm25 weighs the title 5.0 and the content 1.0, so a literal match in a
/// title ranks above the same match in a body. bm25 is lower-is-better, so
/// the score is negated to keep higher-is-better as in semantic search.
pub fn search_full_text(conn: &Connection, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
    let mut stmt = conn.prepare(
        "SELECT n.id, n.title,
              bm25(nodes_fts, 5.0, 1.0) AS score,
              snippet(nodes_fts, 1, '>>>', '<<<', '...', 40) as excerpt
         FROM nodes_fts f
         JOIN nodes n ON n.rowid = f.rowid
        WHERE nodes_fts MATCH ?
        ORDER BY score
        LIMIT ?",
    )?;
    let rows = stmt.query_map(params![escape_fts5_query(query), limit as i64], |r| {
        let score: f64 = r.get(2)?;
        let excerpt: Option<String> = r.get(3)?;
        Ok(SearchResult {
            node_id: r.get(0)?,
            title: r.get(1)?,
            score: -score,
            excerpt: excerpt.unwrap_or_default(),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::super::db::open_db;
    use super::super::nodes::upsert_node;
    use super::*;
    use crate::types::ParsedNode;

    #[test]
    fn title_matches_rank_above_body_matches() {
        let db = open_db(":memory:").unwrap();
        for (id, title, content) in [
            ("body.md", "Other", "mentions zebra once"),
            ("title.md", "Zebra", "nothing here"),
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
        let hits = search_full_text(&db, "zebra", 20).unwrap();
        assert_eq!(hits[0].node_id, "title.md");
        assert!(hits[0].score > 0.0);
        assert!(hits[1].excerpt.contains(">>>zebra<<<"));
    }

    #[test]
    fn hyphenated_queries_do_not_error() {
        let db = open_db(":memory:").unwrap();
        assert!(
            search_full_text(&db, "verify-hyphen-2026", 20)
                .unwrap()
                .is_empty()
        );
    }
}
