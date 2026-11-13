//! The token budget of an embedder model and the adaptive ratchet that
//! shrinks it when chunks fail, persisted in `embedder_capability` and
//! `failed_chunks`.
//!
//! `get_capacity` holds a `&Connection` across the Ollama probe, so its
//! future is not `Send`. A caller that needs a `Send` future runs the three
//! steps itself: `cached_capacity`, `probe_capacity`, `store_capacity`.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use super::js;
use super::metadata_cache::model_hash;
use super::types::Embedder;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacityMethod {
    TokenizerConfig,
    ApiShow,
    Probe,
    Manual,
    Fallback,
}

impl CapacityMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TokenizerConfig => "tokenizer_config",
            Self::ApiShow => "api_show",
            Self::Probe => "probe",
            Self::Manual => "manual",
            Self::Fallback => "fallback",
        }
    }

    /// Parse a stored value. Unknown text reads as `Fallback`.
    pub fn parse(s: &str) -> Self {
        match s {
            "tokenizer_config" => Self::TokenizerConfig,
            "api_show" => Self::ApiShow,
            "probe" => Self::Probe,
            "manual" => Self::Manual,
            _ => Self::Fallback,
        }
    }
}

/// The token budget of a model and how it was found. Size chunks with
/// `chunk_budget_chars`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbedderCapacity {
    pub advertised_max_tokens: u64,
    pub discovered_max_tokens: u64,
    /// floor(0.9 × min(advertised, discovered)): the safe operating budget.
    pub chunk_budget_tokens: u64,
    /// chunk_budget_tokens × the chars-per-token estimate.
    pub chunk_budget_chars: u64,
    pub method: CapacityMethod,
}

/// A conservative chars-per-token estimate. English averages about 3.5,
/// but 2.5 is safe for every language.
pub const CHARS_PER_TOKEN: f64 = 2.5;

/// The advertised limit when nothing better is known.
const FALLBACK_MAX_TOKENS: u64 = 512;

/// The ratchet never pushes `discovered_max_tokens` below this. Below 256
/// tokens chunks shrink to single sentences and retrieval suffers.
pub const MIN_DISCOVERED_TOKENS: u64 = 256;

/// The ONNX embedder's `model_max_length`. `None` when absent or bogus:
/// some tokenizers set a huge value as a "no limit" sentinel.
fn read_transformers_model_max_length(embedder: &dyn Embedder) -> Option<u64> {
    let raw = embedder.model_max_length()?;
    if raw.is_nan() || raw <= 0.0 || raw > 1_000_000.0 {
        return None;
    }
    Some(raw as u64)
}

#[derive(serde::Deserialize)]
struct OllamaShowResponse {
    model_info: Option<serde_json::Map<String, serde_json::Value>>,
    parameters: Option<serde_json::Value>,
}

static NUM_CTX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)num_ctx\s+(\d+)").unwrap());

/// Ask Ollama's `/api/show` for the model's `context_length`, falling back
/// to `num_ctx` in the modelfile parameters. `None` on any failure.
async fn fetch_ollama_context_length(base_url: &str, model_name: &str) -> Option<u64> {
    let res = reqwest::Client::new()
        .post(format!("{base_url}/api/show"))
        .json(&serde_json::json!({ "name": model_name }))
        .send()
        .await
        .ok()?;
    if !res.status().is_success() {
        return None;
    }
    let data: OllamaShowResponse = res.json().await.ok()?;
    if let Some(info) = &data.model_info {
        for (key, val) in info {
            if key.ends_with(".context_length")
                && let Some(n) = val.as_f64().filter(|n| *n > 0.0)
            {
                return Some(n as u64);
            }
        }
    }
    if let Some(serde_json::Value::String(params)) = &data.parameters
        && let Some(m) = NUM_CTX.captures(params)
        && let Ok(n) = m[1].parse::<u64>()
        && n > 0
    {
        return Some(n);
    }
    None
}

struct CapabilityRow {
    advertised: Option<i64>,
    discovered: Option<i64>,
    method: Option<String>,
}

fn load_cached_capability(
    conn: &Connection,
    embedder_id: &str,
    hash: &str,
) -> rusqlite::Result<Option<CapabilityRow>> {
    conn.query_row(
        "SELECT advertised_max_tokens, discovered_max_tokens, method FROM embedder_capability WHERE embedder_id = ? AND model_hash = ?",
        params![embedder_id, hash],
        |r| {
            Ok(CapabilityRow {
                advertised: r.get(0)?,
                discovered: r.get(1)?,
                method: r.get(2)?,
            })
        },
    )
    .optional()
}

fn compute_budget(advertised: u64, discovered: u64, method: CapacityMethod) -> EmbedderCapacity {
    let chunk_budget_tokens = (0.9 * advertised.min(discovered) as f64).floor() as u64;
    let chunk_budget_chars = (chunk_budget_tokens as f64 * CHARS_PER_TOKEN).floor() as u64;
    EmbedderCapacity {
        advertised_max_tokens: advertised,
        discovered_max_tokens: discovered,
        chunk_budget_tokens,
        chunk_budget_chars,
        method,
    }
}

fn positive_int(env: &HashMap<String, String>, key: &str) -> Option<u64> {
    js::parse_int(env.get(key).map(String::as_str))
        .filter(|n| *n > 0)
        .map(|n| n as u64)
}

/// The capacity known without probing: an env override, or a cache hit.
///
/// `OBSIDIAN_BRAIN_MAX_CHUNK_TOKENS_<MODEL_HASH>` overrides one model, and
/// beats the global `OBSIDIAN_BRAIN_MAX_CHUNK_TOKENS`. Both report
/// `method = manual`.
pub fn cached_capacity(
    conn: &Connection,
    embedder: &dyn Embedder,
    env: &HashMap<String, String>,
) -> rusqlite::Result<Option<EmbedderCapacity>> {
    let embedder_id = embedder.model_identifier();
    let hash = model_hash(&embedder_id);

    if let Some(n) = positive_int(env, &format!("OBSIDIAN_BRAIN_MAX_CHUNK_TOKENS_{hash}")) {
        return Ok(Some(compute_budget(n, n, CapacityMethod::Manual)));
    }
    if let Some(n) = positive_int(env, "OBSIDIAN_BRAIN_MAX_CHUNK_TOKENS") {
        return Ok(Some(compute_budget(n, n, CapacityMethod::Manual)));
    }

    if let Some(row) = load_cached_capability(conn, &embedder_id, &hash)?
        && let (Some(a), Some(d)) = (row.advertised, row.discovered)
    {
        let method = row
            .method
            .as_deref()
            .map(CapacityMethod::parse)
            .unwrap_or(CapacityMethod::Fallback);
        return Ok(Some(compute_budget(
            a.max(0) as u64,
            d.max(0) as u64,
            method,
        )));
    }
    Ok(None)
}

/// Probe the model's token limit: the ONNX tokenizer's `model_max_length`,
/// or Ollama's `/api/show`, else 512.
pub async fn probe_capacity(embedder: &dyn Embedder) -> (u64, CapacityMethod) {
    if embedder.provider_name() == "transformers.js" {
        return match read_transformers_model_max_length(embedder) {
            Some(n) => (n, CapacityMethod::TokenizerConfig),
            None => (FALLBACK_MAX_TOKENS, CapacityMethod::Fallback),
        };
    }
    if embedder.provider_name() == "ollama" {
        let embedder_id = embedder.model_identifier();
        let model_name = embedder_id.strip_prefix("ollama:").unwrap_or(&embedder_id);
        let base_url = embedder
            .base_url()
            .or_else(|| std::env::var("OLLAMA_BASE_URL").ok())
            .unwrap_or_else(|| "http://localhost:11434".to_string());
        return match fetch_ollama_context_length(&base_url, model_name).await {
            Some(n) => (n, CapacityMethod::ApiShow),
            None => (FALLBACK_MAX_TOKENS, CapacityMethod::Fallback),
        };
    }
    (FALLBACK_MAX_TOKENS, CapacityMethod::Fallback)
}

/// Persist a probe result and return its budget.
pub fn store_capacity(
    conn: &Connection,
    embedder: &dyn Embedder,
    probed: (u64, CapacityMethod),
) -> rusqlite::Result<EmbedderCapacity> {
    let embedder_id = embedder.model_identifier();
    let (tokens, method) = probed;
    conn.execute(
        "INSERT INTO embedder_capability (embedder_id, model_hash, advertised_max_tokens, discovered_max_tokens, discovered_at, method)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(embedder_id, model_hash) DO UPDATE SET
           advertised_max_tokens = excluded.advertised_max_tokens,
           discovered_max_tokens = excluded.discovered_max_tokens,
           discovered_at = excluded.discovered_at,
           method = excluded.method",
        params![
            embedder_id,
            model_hash(&embedder_id),
            tokens as i64,
            tokens as i64,
            js::now_ms(),
            method.as_str()
        ],
    )?;
    Ok(compute_budget(tokens, tokens, method))
}

/// Get, or probe and cache, the token capacity of `embedder`.
pub async fn get_capacity(
    conn: &Connection,
    embedder: &dyn Embedder,
) -> rusqlite::Result<EmbedderCapacity> {
    get_capacity_with_env(conn, embedder, &js::process_env()).await
}

pub async fn get_capacity_with_env(
    conn: &Connection,
    embedder: &dyn Embedder,
    env: &HashMap<String, String>,
) -> rusqlite::Result<EmbedderCapacity> {
    if let Some(cap) = cached_capacity(conn, embedder, env)? {
        return Ok(cap);
    }
    let probed = probe_capacity(embedder).await;
    store_capacity(conn, embedder, probed)
}

/// Reset `discovered_max_tokens` to `advertised_max_tokens`, so a full
/// reindex starts without the drift of an earlier run. No-op without a row.
pub fn reset_discovered_capacity(
    conn: &Connection,
    embedder: &dyn Embedder,
) -> rusqlite::Result<()> {
    let embedder_id = embedder.model_identifier();
    conn.execute(
        "UPDATE embedder_capability
           SET discovered_max_tokens = advertised_max_tokens,
               discovered_at = ?
         WHERE embedder_id = ? AND model_hash = ?",
        params![js::now_ms(), embedder_id, model_hash(&embedder_id)],
    )?;
    Ok(())
}

/// Record a chunk that failed to embed.
pub fn record_failed_chunk(
    conn: &Connection,
    chunk_id: &str,
    note_id: &str,
    reason: &str,
    error_message: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO failed_chunks (chunk_id, note_id, reason, error_message, failed_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(chunk_id) DO UPDATE SET
           reason = excluded.reason,
           error_message = excluded.error_message,
           failed_at = excluded.failed_at",
        params![chunk_id, note_id, reason, error_message, js::now_ms()],
    )?;
    Ok(())
}

/// Lower the cached `discovered_max_tokens` to half the token count of a
/// chunk that failed, so the next pass tries smaller chunks. One-way: this
/// never raises the bound. The floor is min(MIN_DISCOVERED_TOKENS,
/// advertised), so a tiny model is never credited with more than it has.
pub fn reduce_discovered_max_tokens(
    conn: &Connection,
    embedder: &dyn Embedder,
    failed_chunk_token_count: u64,
) -> rusqlite::Result<()> {
    let embedder_id = embedder.model_identifier();
    let hash = model_hash(&embedder_id);
    let advertised = load_cached_capability(conn, &embedder_id, &hash)?
        .and_then(|r| r.advertised)
        .map(|a| a.max(0) as u64)
        .unwrap_or(FALLBACK_MAX_TOKENS);
    let floor = MIN_DISCOVERED_TOKENS.min(advertised);
    let new_discovered = floor.max(failed_chunk_token_count / 2) as i64;

    conn.execute(
        "INSERT INTO embedder_capability (embedder_id, model_hash, advertised_max_tokens, discovered_max_tokens, discovered_at, method)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(embedder_id, model_hash) DO UPDATE SET
           discovered_max_tokens = MIN(COALESCE(discovered_max_tokens, ?), ?),
           discovered_at = excluded.discovered_at,
           method = excluded.method",
        params![
            embedder_id,
            hash,
            new_discovered,
            new_discovered,
            js::now_ms(),
            "probe",
            new_discovered,
            new_discovered
        ],
    )?;
    Ok(())
}
