//! The bundled model seed, `data/seed-models.json`, compiled into the
//! binary.
//!
//! Schema v2 has three load-bearing fields per model: `maxTokens`,
//! `queryPrefix` and `documentPrefix`. Schema v1 carried more fields; the
//! v1 adapter keeps the same three and drops the rest. The runtime probes
//! `dim` from the loaded model, because the seed's curated dims are not
//! reliable.
//!
//! A bad shape yields an empty map and a warning. The resolver then falls
//! through to the defaults; nothing crashes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

const BUNDLED_SEED: &str = include_str!("../../../../data/seed-models.json");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedEntry {
    /// Effective max input tokens. Always set.
    pub max_tokens: u64,
    /// Query-side prefix, or `None` for a symmetric model.
    pub query_prefix: Option<String>,
    /// Document-side prefix; `""` for models that prefix queries only.
    pub document_prefix: Option<String>,
}

pub type Seed = HashMap<String, SeedEntry>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedMeta {
    pub generated_at: i64,
    pub source: Option<String>,
    pub entries: usize,
}

const SUPPORTED_SCHEMA_VERSIONS: [i64; 2] = [1, 2];

struct Loaded {
    seed: Arc<Seed>,
    meta: Option<SeedMeta>,
}

static CACHED: Mutex<Option<Loaded>> = Mutex::new(None);

fn prefix(v: Option<&Value>) -> Option<Option<String>> {
    match v {
        Some(Value::Null) => Some(None),
        Some(Value::String(s)) => Some(Some(s.clone())),
        _ => None,
    }
}

/// Validate one entry. v1 entries carry extra fields; reading only the
/// three load-bearing ones is the v1 adapter.
pub fn adapt_entry(entry: &Value) -> Option<SeedEntry> {
    let e = entry.as_object()?;
    let max = e.get("maxTokens")?.as_f64()?;
    if !max.is_finite() || max <= 0.0 {
        return None;
    }
    Some(SeedEntry {
        max_tokens: max as u64,
        query_prefix: prefix(e.get("queryPrefix"))?,
        document_prefix: prefix(e.get("documentPrefix"))?,
    })
}

/// Parse a seed file. `None` for meta when the file is unusable.
pub fn parse_seed(text: &str) -> (Seed, Option<SeedMeta>) {
    let mut seed = Seed::new();
    let parsed: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "seed-loader: {err} — proceeding without seed"
            );
            return (seed, None);
        }
    };
    let Some(file) = parsed.as_object() else {
        tracing::warn!("seed-loader: seed JSON has invalid shape — ignoring");
        return (seed, None);
    };
    let version = file.get("$schemaVersion").and_then(Value::as_i64);
    let Some(_) = version.filter(|v| SUPPORTED_SCHEMA_VERSIONS.contains(v)) else {
        let shown = file
            .get("$schemaVersion")
            .map(Value::to_string)
            .unwrap_or_else(|| "?".into());
        tracing::warn!(
            "seed-loader: seed JSON schema version {shown} is not supported (expected one of 1, 2) — ignoring"
        );
        return (seed, None);
    };
    let Some(models) = file.get("models").and_then(Value::as_object) else {
        tracing::warn!("seed-loader: seed JSON has no `models` object — ignoring");
        return (seed, None);
    };
    let mut dropped = 0usize;
    for (id, raw) in models {
        match adapt_entry(raw) {
            Some(entry) => {
                seed.insert(id.clone(), entry);
            }
            None => dropped += 1,
        }
    }
    if dropped > 0 {
        tracing::warn!(
            dropped,
            "seed-loader: {dropped} seed entries skipped due to invalid shape"
        );
    }
    let source = file
        .get("$source")
        .or_else(|| file.get("$mtebRevision"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let meta = SeedMeta {
        generated_at: file
            .get("$generatedAt")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        source,
        entries: seed.len(),
    };
    (seed, Some(meta))
}

fn with_loaded<T>(f: impl FnOnce(&Loaded) -> T) -> T {
    let mut guard = CACHED.lock().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.get_or_insert_with(|| {
        let (seed, meta) = parse_seed(BUNDLED_SEED);
        Loaded {
            seed: Arc::new(seed),
            meta,
        }
    });
    f(loaded)
}

/// The bundled seed, parsed once per process.
pub fn load_seed() -> Arc<Seed> {
    with_loaded(|l| l.seed.clone())
}

/// Freshness of the seed, for diagnostics.
pub fn get_seed_meta() -> Option<SeedMeta> {
    with_loaded(|l| l.meta.clone())
}

/// Reset the in-process cache.
pub fn reset_seed_cache() {
    *CACHED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}
