//! Persistence for resolved model metadata in the metadata columns of
//! `embedder_capability` (`dim`, `query_prefix`, `document_prefix`,
//! `prefix_source`, `base_model`, `size_bytes`, `fetched_at`).
//!
//! The cache lives forever: the cached fields do not change for a given
//! model id. SQL only; the resolver does the orchestration.

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use super::types::PrefixSource;
use crate::util::js;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedMetadata {
    pub model_id: String,
    pub dim: Option<i64>,
    /// Stored in `advertised_max_tokens`.
    pub max_tokens: Option<i64>,
    pub query_prefix: Option<String>,
    pub document_prefix: Option<String>,
    pub prefix_source: PrefixSource,
    pub base_model: Option<String>,
    pub size_bytes: Option<i64>,
    pub fetched_at: Option<i64>,
}

/// The `model_hash` key column: the first 32 hex chars of SHA-256 over the
/// model id, so a renamed model gets a fresh row.
pub fn model_hash(model_id: &str) -> String {
    let digest = Sha256::digest(model_id.as_bytes());
    hex::encode(digest)[..32].to_string()
}

/// Cached metadata for `model_id`, or `None` when absent. A row without
/// `fetched_at` came from the capacity flow alone and reads as a miss.
pub fn load_cached_metadata(
    conn: &Connection,
    model_id: &str,
) -> rusqlite::Result<Option<CachedMetadata>> {
    let row = conn
        .query_row(
            "SELECT advertised_max_tokens, dim, query_prefix, document_prefix,
                    prefix_source, base_model, size_bytes, fetched_at
               FROM embedder_capability
              WHERE embedder_id = ? AND model_hash = ?",
            params![model_id, model_hash(model_id)],
            |r| {
                Ok(CachedMetadata {
                    model_id: model_id.to_string(),
                    max_tokens: r.get(0)?,
                    dim: r.get(1)?,
                    query_prefix: r.get(2)?,
                    document_prefix: r.get(3)?,
                    prefix_source: r
                        .get::<_, Option<String>>(4)?
                        .map(|s| PrefixSource::parse(&s))
                        .unwrap_or(PrefixSource::None),
                    base_model: r.get(5)?,
                    size_bytes: r.get(6)?,
                    fetched_at: r.get(7)?,
                })
            },
        )
        .optional()?;
    Ok(row.filter(|m| m.fetched_at.is_some()))
}

/// Persist metadata for `meta.model_id`. Writes the metadata columns and
/// leaves the capacity columns of an existing row alone.
pub fn upsert_cached_metadata(conn: &Connection, meta: &CachedMetadata) -> rusqlite::Result<()> {
    let now = meta.fetched_at.unwrap_or_else(js::now_ms);
    conn.execute(
        "INSERT INTO embedder_capability (
           embedder_id, model_hash,
           advertised_max_tokens, discovered_max_tokens, discovered_at, method,
           dim, query_prefix, document_prefix, prefix_source, base_model, size_bytes, fetched_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(embedder_id, model_hash) DO UPDATE SET
           advertised_max_tokens = excluded.advertised_max_tokens,
           dim                   = excluded.dim,
           query_prefix          = excluded.query_prefix,
           document_prefix       = excluded.document_prefix,
           prefix_source         = excluded.prefix_source,
           base_model            = excluded.base_model,
           size_bytes            = excluded.size_bytes,
           fetched_at            = excluded.fetched_at",
        params![
            meta.model_id,
            model_hash(&meta.model_id),
            meta.max_tokens,
            // The capacity ratchet starts from the advertised value on the
            // first write; a later write leaves it unchanged.
            meta.max_tokens,
            now,
            "metadata-cache",
            meta.dim,
            meta.query_prefix,
            meta.document_prefix,
            meta.prefix_source.as_str(),
            meta.base_model,
            meta.size_bytes,
            now,
        ],
    )?;
    Ok(())
}
