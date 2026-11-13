//! The end-of-pass check for notes that ended up without any vector.

use rusqlite::Connection;

use crate::store::Result;

const MISSING_SQL: &str = r"
    SELECT id FROM nodes
    WHERE id NOT LIKE '_stub/%' ESCAPE '\'
      AND id NOT IN (
        SELECT DISTINCT c.node_id FROM chunks c
        JOIN chunks_vec v ON c.rowid = v.rowid
        WHERE c.node_id IS NOT NULL
      )
      AND id NOT IN (SELECT DISTINCT note_id FROM failed_chunks WHERE reason = 'no-embeddable-content')";

/// What the check found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RepairCounts {
    /// Real notes without a single chunk vector that are not known to be
    /// empty. Their sync rows are cleared, so the next pass retries them.
    pub unexpected_missing: usize,
    /// Notes recorded as `no-embeddable-content`. They are not retried until
    /// the file changes, which would only loop.
    pub no_content_total: usize,
}

/// Count the notes without vectors and clear their sync rows, so the next
/// change-driven pass embeds them again. Notes with no embeddable content
/// are counted apart and left alone.
pub fn repair_missing_embeddings(conn: &Connection) -> Result<RepairCounts> {
    let unexpected_missing: i64 = conn.query_row(
        &format!("SELECT COUNT(*) AS n FROM ({MISSING_SQL})"),
        [],
        |r| r.get(0),
    )?;
    if unexpected_missing > 0 {
        tracing::warn!(
            count = unexpected_missing,
            "{unexpected_missing} note(s) failed to embed — clearing their sync timestamps so they retry on the next change-driven indexer pass"
        );
        conn.execute(
            &format!("DELETE FROM sync WHERE path IN ({MISSING_SQL})"),
            [],
        )?;
    }

    let no_content_total: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT note_id) AS n FROM failed_chunks WHERE reason = 'no-embeddable-content'",
        [],
        |r| r.get(0),
    )?;
    if no_content_total > 0 {
        tracing::info!(
            count = no_content_total,
            "{no_content_total} notes have no embeddable content (empty / frontmatter-only / sub-minChunkChars body) — recorded as 'no-embeddable-content' in failed_chunks; will not retry until the file changes"
        );
    }

    Ok(RepairCounts {
        unexpected_missing: unexpected_missing as usize,
        no_content_total: no_content_total as usize,
    })
}
