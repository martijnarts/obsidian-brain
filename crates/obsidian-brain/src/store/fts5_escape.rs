//! Escaping user queries for FTS5 MATCH.

/// Escape a user query for an FTS5 MATCH clause.
///
/// FTS5 reads `- + * " ( ) : / ^ $` as operators, so `verify-hyphen-2026`
/// parses as `verify NOT hyphen:2026` and fails. A query of only ASCII
/// word characters, whitespace and `*` passes through, so AND, OR, NEAR
/// and prefix `*` keep working. Anything else is phrase-quoted, with inner
/// double quotes doubled.
pub fn escape_fts5_query(query: &str) -> String {
    let plain = query.chars().all(|c| {
        c.is_ascii_alphanumeric() || c == '_' || c == '*' || crate::util::js::is_js_whitespace(c)
    });
    if query.is_empty() || plain {
        return query.to_string();
    }
    format!("\"{}\"", query.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrase_quotes_special_characters() {
        assert_eq!(
            escape_fts5_query("verify-hyphen-20260422"),
            "\"verify-hyphen-20260422\""
        );
        assert_eq!(escape_fts5_query("title:foo"), "\"title:foo\"");
        assert_eq!(
            escape_fts5_query("NEAR(foo bar, 5)"),
            "\"NEAR(foo bar, 5)\""
        );
    }

    #[test]
    fn passes_plain_queries_through() {
        assert_eq!(escape_fts5_query("foo bar"), "foo bar");
        assert_eq!(escape_fts5_query("foo AND bar"), "foo AND bar");
        assert_eq!(escape_fts5_query("prefix*"), "prefix*");
        assert_eq!(escape_fts5_query(""), "");
    }

    #[test]
    fn doubles_inner_double_quotes() {
        assert_eq!(escape_fts5_query("say \"hello\""), "\"say \"\"hello\"\"\"");
    }

    #[test]
    fn phrase_quotes_non_ascii() {
        assert_eq!(escape_fts5_query("café"), "\"café\"");
        assert_eq!(escape_fts5_query("日本語"), "\"日本語\"");
        assert_eq!(escape_fts5_query("party 🎉"), "\"party 🎉\"");
        assert_eq!(escape_fts5_query("café notes"), "\"café notes\"");
    }

    #[test]
    fn escaped_queries_parse_as_match_expressions() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE VIRTUAL TABLE t USING fts5(body);
             INSERT INTO t(body) VALUES ('café notes'), ('日本語 tests'), ('plain text');",
        )
        .unwrap();
        for raw in [
            "café",
            "日本語",
            "café notes",
            "verify-hyphen-2026",
            "title:foo",
        ] {
            let escaped = escape_fts5_query(raw);
            let mut stmt = db.prepare("SELECT rowid FROM t WHERE t MATCH ?").unwrap();
            let rows: rusqlite::Result<Vec<i64>> =
                stmt.query_map([&escaped], |r| r.get(0)).unwrap().collect();
            assert!(rows.is_ok(), "{raw} -> {escaped}");
        }
    }
}
