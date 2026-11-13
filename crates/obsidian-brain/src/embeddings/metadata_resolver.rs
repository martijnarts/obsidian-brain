//! The model metadata resolution chain.
//!
//! Order:
//!   0. A complete user override (all three fields) short-circuits.
//!   1. `embedder_capability` cache hit: return it. A stale row with both
//!      prefixes NULL is promoted from the seed when the seed has the model.
//!   2. Seed hit: write it to the cache and return it.
//!   3. Embedder loaded: its probed dim, 512 tokens, no prefixes.
//!   4. Safe defaults, with a warning. Boot continues.
//!
//! User overrides patch whatever the chain resolved. Dependencies are
//! injected, so tests can drive every step.

use rusqlite::Connection;
use serde::Serialize;

use super::metadata_cache::{CachedMetadata, load_cached_metadata, upsert_cached_metadata};
use super::overrides::{ModelOverride, Overrides, load_overrides};
use super::seed_loader::{Seed, SeedEntry, load_seed};
use super::types::{Embedder, EmbedderMetadata, PrefixSource};
use crate::util::js;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResolvedFrom {
    Cache,
    Seed,
    EmbedderProbe,
    Fallback,
}

/// Always produced: the chain ends in safe defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedMetadata {
    pub model_id: String,
    pub dim: Option<i64>,
    pub max_tokens: u64,
    pub query_prefix: String,
    pub document_prefix: String,
    pub prefix_source: PrefixSource,
    pub base_model: Option<String>,
    pub size_bytes: Option<i64>,
    pub resolved_from: ResolvedFrom,
    /// True when a user override patched any field.
    pub override_applied: bool,
}

impl ResolvedMetadata {
    /// The shape `Embedder::set_metadata` takes.
    pub fn to_embedder_metadata(&self) -> EmbedderMetadata {
        EmbedderMetadata {
            model_id: self.model_id.clone(),
            dim: self.dim.and_then(|d| usize::try_from(d).ok()),
            max_tokens: self.max_tokens,
            query_prefix: self.query_prefix.clone(),
            document_prefix: self.document_prefix.clone(),
            prefix_source: self.prefix_source,
            base_model: self.base_model.clone(),
            size_bytes: self.size_bytes,
        }
    }
}

pub struct ResolverDeps<'a> {
    pub conn: &'a Connection,
    /// When present, its dimensions feed the probe step.
    pub embedder: Option<&'a dyn Embedder>,
    /// Defaults to the bundled seed.
    pub seed: Option<&'a Seed>,
    /// Defaults to the user's override file.
    pub overrides: Option<&'a Overrides>,
}

impl<'a> ResolverDeps<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self {
            conn,
            embedder: None,
            seed: None,
            overrides: None,
        }
    }
}

const FALLBACK_MAX_TOKENS: u64 = 512;

/// The seed keys Ollama models by their bare tag, while the Ollama
/// embedder's identifier carries an `ollama:` prefix. Cache rows keep the
/// prefixed id; only the seed lookup is normalised.
fn seed_key(model_id: &str) -> &str {
    model_id.strip_prefix("ollama:").unwrap_or(model_id)
}

/// The full chain. Used after `embedder.init()`.
pub fn resolve_model_metadata(
    model_id: &str,
    deps: &ResolverDeps<'_>,
) -> rusqlite::Result<ResolvedMetadata> {
    let seed_owned;
    let seed = match deps.seed {
        Some(s) => s,
        None => {
            seed_owned = load_seed();
            &*seed_owned
        }
    };
    let overrides_owned;
    let overrides = match deps.overrides {
        Some(o) => o,
        None => {
            overrides_owned = load_overrides();
            &*overrides_owned
        }
    };
    let over = overrides.get(model_id);

    // Step 0: a complete override fully specifies the model.
    if let Some(complete) = over.filter(|o| is_complete_override(o)) {
        let synthetic = override_to_cached(model_id, complete);
        upsert_cached_metadata(deps.conn, &synthetic)?;
        return Ok(materialise(&synthetic, ResolvedFrom::Cache, over));
    }

    // Step 1: cache hit.
    if let Some(cached) = load_cached_metadata(deps.conn, model_id)? {
        if let Some(promoted) = promote_from_seed_if_stale(&cached, seed, deps.conn)? {
            return Ok(materialise(&promoted, ResolvedFrom::Seed, over));
        }
        return Ok(materialise(&cached, ResolvedFrom::Cache, over));
    }

    // Step 2: seed lookup.
    if let Some(entry) = seed.get(seed_key(model_id)) {
        let from_seed = seed_entry_to_cached(model_id, entry);
        upsert_cached_metadata(deps.conn, &from_seed)?;
        return Ok(materialise(&from_seed, ResolvedFrom::Seed, over));
    }

    tracing::warn!(
        model_id,
        "metadata-resolver: no cached or seeded metadata for {model_id}; falling back"
    );

    // Step 3: the loaded embedder's probed dim.
    if let Some(embedder) = deps.embedder {
        let probed = embedder_probe_to_cached(model_id, embedder);
        upsert_cached_metadata(deps.conn, &probed)?;
        return Ok(materialise(&probed, ResolvedFrom::EmbedderProbe, over));
    }

    // Step 4: safe defaults.
    let fallback = safe_defaults(model_id);
    upsert_cached_metadata(deps.conn, &fallback)?;
    Ok(materialise(&fallback, ResolvedFrom::Fallback, over))
}

/// Cache and seed only. `None` when neither has the model; the caller then
/// skips the prefix-strategy check.
pub fn resolve_model_metadata_sync(
    model_id: &str,
    deps: &ResolverDeps<'_>,
) -> rusqlite::Result<Option<ResolvedMetadata>> {
    let overrides_owned;
    let overrides = match deps.overrides {
        Some(o) => o,
        None => {
            overrides_owned = load_overrides();
            &*overrides_owned
        }
    };
    let over = overrides.get(model_id);
    let seed_owned;
    let seed = match deps.seed {
        Some(s) => s,
        None => {
            seed_owned = load_seed();
            &*seed_owned
        }
    };
    if let Some(cached) = load_cached_metadata(deps.conn, model_id)? {
        if let Some(promoted) = promote_from_seed_if_stale(&cached, seed, deps.conn)? {
            return Ok(Some(materialise(&promoted, ResolvedFrom::Seed, over)));
        }
        return Ok(Some(materialise(&cached, ResolvedFrom::Cache, over)));
    }
    if let Some(entry) = seed.get(seed_key(model_id)) {
        let from_seed = seed_entry_to_cached(model_id, entry);
        upsert_cached_metadata(deps.conn, &from_seed)?;
        return Ok(Some(materialise(&from_seed, ResolvedFrom::Seed, over)));
    }
    Ok(None)
}

/// Older installs and the probe fallback wrote rows with both prefixes
/// NULL. Such a row would short-circuit every boot at the cache step and
/// leave asymmetric models without their query prefix, so when the seed
/// has the model, the seed row replaces it.
fn promote_from_seed_if_stale(
    cached: &CachedMetadata,
    seed: &Seed,
    conn: &Connection,
) -> rusqlite::Result<Option<CachedMetadata>> {
    if cached.query_prefix.is_some() || cached.document_prefix.is_some() {
        return Ok(None);
    }
    let Some(entry) = seed.get(seed_key(&cached.model_id)) else {
        return Ok(None);
    };
    let from_seed = seed_entry_to_cached(&cached.model_id, entry);
    upsert_cached_metadata(conn, &from_seed)?;
    Ok(Some(from_seed))
}

/// Overlay a user override on the resolved row. A `null` prefix in the
/// override clears it; an absent one leaves the row's value.
fn materialise(
    meta: &CachedMetadata,
    resolved_from: ResolvedFrom,
    over: Option<&ModelOverride>,
) -> ResolvedMetadata {
    let base_max = meta
        .max_tokens
        .and_then(|n| u64::try_from(n).ok())
        .unwrap_or(FALLBACK_MAX_TOKENS);
    let base_query = meta.query_prefix.clone().unwrap_or_default();
    let base_document = meta.document_prefix.clone().unwrap_or_default();

    let over_max = over.and_then(|o| o.max_tokens);
    let over_query = over
        .and_then(|o| o.query_prefix.as_ref())
        .map(|p| p.clone().unwrap_or_default());
    let over_document = over
        .and_then(|o| o.document_prefix.as_ref())
        .map(|p| p.clone().unwrap_or_default());
    let override_applied = over_max.is_some() || over_query.is_some() || over_document.is_some();

    ResolvedMetadata {
        model_id: meta.model_id.clone(),
        dim: meta.dim,
        max_tokens: over_max.unwrap_or(base_max),
        query_prefix: over_query.unwrap_or(base_query),
        document_prefix: over_document.unwrap_or(base_document),
        prefix_source: if override_applied {
            PrefixSource::Override
        } else {
            meta.prefix_source
        },
        base_model: meta.base_model.clone(),
        size_bytes: meta.size_bytes,
        resolved_from,
        override_applied,
    }
}

/// All three load-bearing fields set. A `null` prefix counts as set.
fn is_complete_override(o: &ModelOverride) -> bool {
    o.max_tokens.is_some() && o.query_prefix.is_some() && o.document_prefix.is_some()
}

fn row(model_id: &str) -> CachedMetadata {
    CachedMetadata {
        model_id: model_id.to_string(),
        dim: None,
        max_tokens: Some(FALLBACK_MAX_TOKENS as i64),
        query_prefix: None,
        document_prefix: None,
        prefix_source: PrefixSource::Fallback,
        base_model: None,
        size_bytes: None,
        fetched_at: Some(js::now_ms()),
    }
}

fn override_to_cached(model_id: &str, o: &ModelOverride) -> CachedMetadata {
    CachedMetadata {
        max_tokens: o.max_tokens.map(|n| n as i64),
        query_prefix: o.query_prefix.clone().flatten(),
        document_prefix: o.document_prefix.clone().flatten(),
        prefix_source: PrefixSource::Override,
        ..row(model_id)
    }
}

/// The seed carries no dim; the runtime probes it.
fn seed_entry_to_cached(model_id: &str, entry: &SeedEntry) -> CachedMetadata {
    CachedMetadata {
        max_tokens: Some(entry.max_tokens as i64),
        query_prefix: entry.query_prefix.clone(),
        document_prefix: entry.document_prefix.clone(),
        prefix_source: PrefixSource::Seed,
        ..row(model_id)
    }
}

fn embedder_probe_to_cached(model_id: &str, embedder: &dyn Embedder) -> CachedMetadata {
    CachedMetadata {
        dim: embedder.dimensions().ok().map(|d| d as i64),
        ..row(model_id)
    }
}

fn safe_defaults(model_id: &str) -> CachedMetadata {
    row(model_id)
}
