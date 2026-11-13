//! The startup compatibility check between the stored index and the live
//! embedder: model and dim changes, swapped weights, the empty-chunks and
//! FTS tokenizer upgrades, and prefix-strategy changes.
//!
//! The TypeScript server also ran a chain of in-place schema migrations
//! here. The Rust index rebuilds itself when its layout version changes
//! (see `store::db::open_db`), so that chain has no counterpart.

use rusqlite::Connection;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::embeddings::Embedder;
use crate::embeddings::metadata_cache::{
    CachedMetadata, load_cached_metadata, upsert_cached_metadata,
};
use crate::embeddings::overrides::load_overrides;
use crate::embeddings::types::PrefixSource;
use crate::store::chunks::count_chunks;
use crate::store::db::{
    EXPECTED_FTS_TOKENIZE, SCHEMA_VERSION, current_fts_tokenize, drop_embedding_state,
    ensure_vec_tables, rebuild_full_text_index,
};
use crate::store::metadata::{get_metadata, set_metadata};
use crate::store::nodes::all_node_ids;
use crate::util::js;

/// Bumped whenever the prefix table changes, so a prefix-table update
/// forces a reindex even when the prefix strings are identical.
const PREFIX_STRATEGY_VERSION: u32 = 2;

/// A stable hash of the embedder's prefix strategy, or `""` for a
/// symmetric model or an embedder without metadata yet. The metadata is
/// pushed onto the embedder before `bootstrap` runs; when it is missing
/// (a model the seed does not know and HF unreachable), the trigger no-ops
/// and the next boot picks the prefix up.
fn compute_prefix_strategy(embedder: &dyn Embedder) -> String {
    let Some(meta) = embedder.get_metadata() else {
        return String::new();
    };
    let (q, d) = (&meta.query_prefix, &meta.document_prefix);
    if q.is_empty() && d.is_empty() {
        return String::new();
    }
    let digest = Sha256::digest(format!("{q}|{d}|v{PREFIX_STRATEGY_VERSION}").as_bytes());
    hex::encode(digest)[..16].to_string()
}

/// The outcome of a bootstrap check. `reasons` say why a reindex starts;
/// the server logs them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapResult {
    pub needs_reindex: bool,
    pub reasons: Vec<String>,
}

/// Check the index against the embedder and queue any automatic
/// migration. Runs before the first index call, after `init()`.
///
///   1. A new index: stamp the metadata, no reindex.
///   2. A model or dim change: drop the embedding state and reindex.
///   3. Nodes without chunks, or an old FTS tokenizer: rebuild FTS in
///      place and reindex so `chunks_vec` fills.
pub fn bootstrap(conn: &Connection, embedder: &dyn Embedder) -> anyhow::Result<BootstrapResult> {
    let mut reasons = Vec::new();
    let mut needs_reindex = false;
    let current_model = embedder.model_identifier();
    let current_dim = embedder.dimensions()?;

    let stored_model = get_metadata(conn, "embedding_model")?.filter(|m| !m.is_empty());
    let stored_dim = js::number(
        get_metadata(conn, "embedding_dim")?
            .as_deref()
            .or(Some("0")),
    );

    match &stored_model {
        // First metadata write: a new index, or nodes from some other path.
        // The chunk and FTS checks below still run.
        None => {
            ensure_vec_tables(conn, current_dim)?;
            set_metadata(conn, "embedding_model", &current_model)?;
            set_metadata(conn, "embedding_dim", &current_dim.to_string())?;
            set_metadata(conn, "schema_version", &SCHEMA_VERSION.to_string())?;
            set_metadata(conn, "embedder_provider", embedder.provider_name())?;
        }
        Some(stored) if *stored != current_model || stored_dim != current_dim as f64 => {
            // Drop everything embedded against the old model and reindex.
            reasons.push(format!(
                "embedding model changed: {stored} → {current_model} — re-embedding all notes"
            ));
            needs_reindex = true;
            drop_embedding_state(conn)?;
            ensure_vec_tables(conn, current_dim)?;
            set_metadata(conn, "embedding_model", &current_model)?;
            set_metadata(conn, "embedding_dim", &current_dim.to_string())?;
            set_metadata(conn, "embedder_provider", embedder.provider_name())?;
        }
        // Same identity, but the vec tables may still be missing.
        Some(_) => ensure_vec_tables(conn, current_dim)?,
    }

    // Identity hash: the model id can stay the same while the weights swap
    // (`ollama pull` of a moved tag). Only Ollama reports one. An absent
    // stored hash is stamped without a reindex; an absent live hash (Ollama
    // unreachable at init) skips the comparison.
    let stored_hash = get_metadata(conn, "embedder_identity_hash")?.filter(|h| !h.is_empty());
    if let Some(current_hash) = embedder.identity_hash() {
        match stored_hash {
            Some(stored) if stored != current_hash => {
                reasons.push(format!(
                    "model weights for {current_model} were updated (probably an `ollama pull`) — re-embedding to match the new weights"
                ));
                needs_reindex = true;
                drop_embedding_state(conn)?;
                ensure_vec_tables(conn, current_dim)?;
                set_metadata(conn, "embedder_identity_hash", &current_hash)?;
            }
            Some(_) => {}
            None => set_metadata(conn, "embedder_identity_hash", &current_hash)?,
        }
    }

    // Ollama: refresh the cached dim and max tokens from the live
    // `/api/show` values on every boot. A user override of max tokens wins
    // over the live value. Prefix fields are left to the resolver chain.
    if embedder.provider_name() == "ollama"
        && let Some(live_ctx) = embedder.context_length().filter(|n| *n > 0)
    {
        let overrides = load_overrides();
        let new_max_tokens = overrides
            .get(&current_model)
            .and_then(|o| o.max_tokens)
            .unwrap_or(live_ctx) as i64;
        let new_dim = current_dim as i64;
        let existing = load_cached_metadata(conn, &current_model)?;
        let dirty = match &existing {
            None => true,
            Some(e) => e.max_tokens != Some(new_max_tokens) || e.dim != Some(new_dim),
        };
        if dirty {
            upsert_cached_metadata(
                conn,
                &CachedMetadata {
                    model_id: current_model.clone(),
                    dim: Some(new_dim),
                    max_tokens: Some(new_max_tokens),
                    query_prefix: existing.as_ref().and_then(|e| e.query_prefix.clone()),
                    document_prefix: existing.as_ref().and_then(|e| e.document_prefix.clone()),
                    prefix_source: existing
                        .as_ref()
                        .map_or(PrefixSource::None, |e| e.prefix_source),
                    base_model: existing.as_ref().and_then(|e| e.base_model.clone()),
                    size_bytes: existing.as_ref().and_then(|e| e.size_bytes),
                    fetched_at: Some(js::now_ms()),
                },
            )?;
            if let Some(e) = &existing {
                let show = |v: Option<i64>| v.map_or("null".to_string(), |n| n.to_string());
                reasons.push(format!(
                    "Ollama live values refreshed: dim={new_dim} max-tokens={new_max_tokens} (prev cached: dim={} max-tokens={})",
                    show(e.dim),
                    show(e.max_tokens)
                ));
            }
        }
    }

    // Nodes but no chunks: an index built before paragraph-level chunks.
    let has_nodes = !all_node_ids(conn)?.is_empty();
    if has_nodes && count_chunks(conn)? == 0 {
        reasons.push("first-time paragraph-level indexing — re-embedding all notes".into());
        needs_reindex = true;
    }

    // An FTS tokenizer change is swapped in place: the bodies did not
    // change, only how they are tokenised.
    let fts_tok = current_fts_tokenize(conn)?;
    if has_nodes && fts_tok.as_deref() != Some(EXPECTED_FTS_TOKENIZE) {
        reasons.push(format!(
            "full-text search tokenization updated ({} → {EXPECTED_FTS_TOKENIZE}) — rebuilding the search index",
            fts_tok.as_deref().unwrap_or("none")
        ));
        rebuild_full_text_index(conn)?;
    }

    // Prefix strategy: only compared against a stored one. A first boot
    // stamps it; the index is empty then.
    let stored_strategy = get_metadata(conn, "embedder_prefix_strategy")?.unwrap_or_default();
    let current_strategy = compute_prefix_strategy(embedder);
    if stored_strategy != current_strategy {
        if stored_model.is_none() {
            // First boot: nothing to reindex yet.
        } else if current_strategy.is_empty() {
            // Now symmetric: old vectors may carry a baked-in prefix.
            reasons.push(
                "embedding model now treats queries and documents the same way — re-embedding to match"
                    .into(),
            );
            needs_reindex = true;
        } else {
            reasons.push(format!(
                "embedding model {current_model} uses different query/document prefixes than before — re-embedding for accurate search"
            ));
            needs_reindex = true;
        }
        set_metadata(conn, "embedder_prefix_strategy", &current_strategy)?;
    }

    Ok(BootstrapResult {
        needs_reindex,
        reasons,
    })
}
