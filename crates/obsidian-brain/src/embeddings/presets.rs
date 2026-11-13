//! The named embedding presets and the one resolver every consumer calls.
//!
//! Precedence in `resolve_preset_config`, highest first:
//!   1. `EMBEDDING_MODEL` set: use it raw; the provider is
//!      `EMBEDDING_PROVIDER` if set, else `transformers`.
//!   2. `EMBEDDING_PROVIDER` and `EMBEDDING_PRESET` disagree on the
//!      provider: the provider wins, the preset's model carries over, and a
//!      mismatch warning fires once.
//!   3. `EMBEDDING_PRESET` set: the preset's (provider, model) pair.
//!      Deprecated aliases resolve to their canonical name with a warning.
//!   4. `EMBEDDING_PROVIDER` alone: the provider's default model.
//!   5. Nothing set: `DEFAULT_PRESET`.
//!
//! Per-model metadata (dim, max tokens, prefixes) lives in the metadata
//! resolver, not here.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use crate::util::js;

pub const DEFAULT_PRESET: &str = "english";
pub const DEFAULT_OLLAMA_MODEL: &str = "nomic-embed-text";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingProvider {
    Transformers,
    Ollama,
}

impl EmbeddingProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transformers => "transformers",
            Self::Ollama => "ollama",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingPreset {
    pub model: &'static str,
    pub provider: EmbeddingProvider,
}

/// The friendly-name registry, in declaration order.
pub const EMBEDDING_PRESETS: &[(&str, EmbeddingPreset)] = &[
    (
        "english",
        EmbeddingPreset {
            model: "Xenova/bge-small-en-v1.5",
            provider: EmbeddingProvider::Transformers,
        },
    ),
    (
        "english-fast",
        EmbeddingPreset {
            model: "MongoDB/mdbr-leaf-ir",
            provider: EmbeddingProvider::Transformers,
        },
    ),
    (
        "english-quality",
        EmbeddingPreset {
            model: "Xenova/bge-base-en-v1.5",
            provider: EmbeddingProvider::Transformers,
        },
    ),
    (
        "multilingual",
        EmbeddingPreset {
            model: "Xenova/multilingual-e5-small",
            provider: EmbeddingProvider::Transformers,
        },
    ),
    (
        "multilingual-quality",
        EmbeddingPreset {
            model: "Xenova/multilingual-e5-base",
            provider: EmbeddingProvider::Transformers,
        },
    ),
    (
        "multilingual-ollama",
        EmbeddingPreset {
            model: "qwen3-embedding:0.6b",
            provider: EmbeddingProvider::Ollama,
        },
    ),
];

/// Deprecated aliases and their canonical preset. `balanced` is a model
/// change, not a rename: it now means bge-small-en-v1.5.
pub const DEPRECATED_PRESET_ALIASES: &[(&str, &str)] =
    &[("fastest", "english-fast"), ("balanced", "english")];

pub fn preset(name: &str) -> Option<EmbeddingPreset> {
    EMBEDDING_PRESETS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, p)| *p)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PresetSource {
    EnvModel,
    EnvPreset,
    EnvProvider,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetConfig {
    pub provider: EmbeddingProvider,
    pub model: String,
    /// `None` when `EMBEDDING_MODEL` named the model directly.
    pub preset_name: Option<&'static str>,
    pub source: PresetSource,
}

static WARNED_ALIASES: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static WARNED_MULTILINGUAL_QUALITY: AtomicBool = AtomicBool::new(false);
static WARNED_PROVIDER_MISMATCH: AtomicBool = AtomicBool::new(false);

fn emit_alias_warning(raw_name: &str, canonical: &str) {
    let mut guard = WARNED_ALIASES.lock().unwrap_or_else(|e| e.into_inner());
    if !guard
        .get_or_insert_with(HashSet::new)
        .insert(raw_name.to_string())
    {
        return;
    }
    if raw_name == "balanced" {
        tracing::warn!(
            raw_name,
            canonical,
            "EMBEDDING_PRESET=\"balanced\" is deprecated. It now resolves to \"english\" (Xenova/bge-small-en-v1.5) — a different model than the old Xenova/all-MiniLM-L6-v2. Your vault will re-embed once on next boot. To keep the old model explicitly, set EMBEDDING_MODEL=Xenova/all-MiniLM-L6-v2."
        );
    } else {
        tracing::warn!(
            raw_name,
            canonical,
            "EMBEDDING_PRESET=\"{raw_name}\" is deprecated and has been renamed to \"{canonical}\". Please update your configuration."
        );
    }
}

fn emit_multilingual_quality_warning() {
    if WARNED_MULTILINGUAL_QUALITY.swap(true, Ordering::SeqCst) {
        return;
    }
    tracing::warn!(
        "⚠ EMBEDDING_PRESET=\"multilingual-quality\" (Xenova/multilingual-e5-base) has a known token_type_ids bug in transformers.js for inputs > ~400 words (transformers.js#267). Notes that hit this bug are recorded in the failed_chunks table and surfaced via the index_status tool. For lossless multilingual quality, prefer EMBEDDING_PRESET=multilingual-ollama (qwen3-embedding:0.6b via Ollama, 32768 ctx, MTEB multi 64.3 vs e5-base's 59.0). For smaller-but-tolerant transformers.js: EMBEDDING_PRESET=multilingual."
    );
}

fn emit_provider_mismatch_warning(
    over: EmbeddingProvider,
    preset_name: &str,
    preset_provider: EmbeddingProvider,
    preset_model: &str,
) {
    if WARNED_PROVIDER_MISMATCH.swap(true, Ordering::SeqCst) {
        return;
    }
    let (o, p) = (over.as_str(), preset_provider.as_str());
    tracing::warn!(
        r#override = o,
        preset_name,
        preset_provider = p,
        preset_model,
        "⚠ EMBEDDING_PROVIDER='{o}' overrides EMBEDDING_PRESET='{preset_name}' which declares provider='{p}'. Will attempt model='{preset_model}' on provider='{o}' — likely fails unless that model exists on the chosen provider. Either remove EMBEDDING_PROVIDER, switch to a preset that declares provider='{o}', or set EMBEDDING_MODEL explicitly."
    );
}

fn parse_explicit_provider(
    env: &HashMap<String, String>,
) -> anyhow::Result<Option<EmbeddingProvider>> {
    let Some(raw) = env.get("EMBEDDING_PROVIDER") else {
        return Ok(None);
    };
    let v = js::trim(raw);
    if v.is_empty() {
        return Ok(None);
    }
    match v.to_lowercase().as_str() {
        "transformers" => Ok(Some(EmbeddingProvider::Transformers)),
        "ollama" => Ok(Some(EmbeddingProvider::Ollama)),
        _ => anyhow::bail!(
            "Unknown EMBEDDING_PROVIDER='{raw}'. Valid providers: transformers, ollama."
        ),
    }
}

fn resolve_canonical_preset(raw_name: &str) -> anyhow::Result<&'static str> {
    if let Some((_, canonical)) = DEPRECATED_PRESET_ALIASES
        .iter()
        .find(|(a, _)| *a == raw_name)
    {
        emit_alias_warning(raw_name, canonical);
        return Ok(canonical);
    }
    if let Some((name, _)) = EMBEDDING_PRESETS.iter().find(|(n, _)| *n == raw_name) {
        return Ok(name);
    }
    let valid: Vec<&str> = EMBEDDING_PRESETS.iter().map(|(n, _)| *n).collect();
    anyhow::bail!(
        "Unknown EMBEDDING_PRESET='{raw_name}'. Valid presets: {}. Or set EMBEDDING_MODEL to a specific HF model id (power-user path).",
        valid.join(", ")
    )
}

/// The single resolver. Returns provider and model together, so they
/// cannot drift apart.
pub fn resolve_preset_config(env: &HashMap<String, String>) -> anyhow::Result<PresetConfig> {
    let explicit_provider = parse_explicit_provider(env)?;

    // (1) EMBEDDING_MODEL set: use it raw.
    if let Some(model) = js::env_value(env, "EMBEDDING_MODEL") {
        return Ok(PresetConfig {
            provider: explicit_provider.unwrap_or(EmbeddingProvider::Transformers),
            model: model.to_string(),
            preset_name: None,
            source: PresetSource::EnvModel,
        });
    }

    // (2/3) A preset set in the environment.
    if let Some(raw) = js::env_value(env, "EMBEDDING_PRESET") {
        let raw_name = raw.to_lowercase();
        let canonical = resolve_canonical_preset(&raw_name)?;
        if canonical == "multilingual-quality" {
            emit_multilingual_quality_warning();
        }
        let p = preset(canonical).expect("canonical preset exists");
        let provider = match explicit_provider {
            None => p.provider,
            Some(over) if over == p.provider => over,
            Some(over) => {
                emit_provider_mismatch_warning(over, canonical, p.provider, p.model);
                over
            }
        };
        return Ok(PresetConfig {
            provider,
            model: p.model.to_string(),
            preset_name: Some(canonical),
            source: PresetSource::EnvPreset,
        });
    }

    // (4) A provider without a preset or model: the provider's default.
    let def = preset(DEFAULT_PRESET).expect("default preset exists");
    match explicit_provider {
        Some(EmbeddingProvider::Ollama) => Ok(PresetConfig {
            provider: EmbeddingProvider::Ollama,
            model: DEFAULT_OLLAMA_MODEL.to_string(),
            preset_name: None,
            source: PresetSource::EnvProvider,
        }),
        Some(EmbeddingProvider::Transformers) => Ok(PresetConfig {
            provider: EmbeddingProvider::Transformers,
            model: def.model.to_string(),
            preset_name: Some(DEFAULT_PRESET),
            source: PresetSource::EnvProvider,
        }),
        // (5) Nothing set.
        None => Ok(PresetConfig {
            provider: def.provider,
            model: def.model.to_string(),
            preset_name: Some(DEFAULT_PRESET),
            source: PresetSource::Default,
        }),
    }
}

pub fn resolve_embedding_model(env: &HashMap<String, String>) -> anyhow::Result<String> {
    Ok(resolve_preset_config(env)?.model)
}

pub fn resolve_embedding_provider(
    env: &HashMap<String, String>,
) -> anyhow::Result<EmbeddingProvider> {
    Ok(resolve_preset_config(env)?.provider)
}

/// Test helper: let the one-shot warnings fire again.
#[doc(hidden)]
pub fn reset_warnings() {
    *WARNED_ALIASES.lock().unwrap_or_else(|e| e.into_inner()) = None;
    WARNED_MULTILINGUAL_QUALITY.store(false, Ordering::SeqCst);
    WARNED_PROVIDER_MISMATCH.store(false, Ordering::SeqCst);
}
