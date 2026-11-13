//! Builds the embedder for the resolved (provider, model) pair, and shares
//! one embedder between every vault.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::embedder::{TransformersEmbedder, TransformersEmbedderOptions};
use super::ollama::OllamaEmbedder;
use super::presets::{EmbeddingProvider, resolve_preset_config};
use super::types::Embedder;
use crate::util::js;

/// Where model files are cached: `TRANSFORMERS_CACHE` when set, else
/// `<data dir>/models`.
pub fn model_cache_dir(data_dir: &Path) -> PathBuf {
    model_cache_dir_from(data_dir, &js::process_env())
}

pub fn model_cache_dir_from(data_dir: &Path, env: &HashMap<String, String>) -> PathBuf {
    js::env_value(env, "TRANSFORMERS_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join("models"))
}

/// Build the embedder that `EMBEDDING_PRESET`, `EMBEDDING_PROVIDER` and
/// `EMBEDDING_MODEL` select. It still needs `init()`. `data_dir` is the
/// top-level data dir, not a vault's.
///
/// The Ollama embedder reads `OLLAMA_BASE_URL`, `OLLAMA_EMBEDDING_DIM`
/// (which spares the probe embed) and `OLLAMA_NUM_CTX`.
pub fn create_embedder(data_dir: &Path) -> anyhow::Result<Arc<dyn Embedder>> {
    create_embedder_with_env(data_dir, &js::process_env())
}

pub fn create_embedder_with_env(
    data_dir: &Path,
    env: &HashMap<String, String>,
) -> anyhow::Result<Arc<dyn Embedder>> {
    let cfg = resolve_preset_config(env)?;
    match cfg.provider {
        EmbeddingProvider::Ollama => {
            let url = env
                .get("OLLAMA_BASE_URL")
                .map(String::as_str)
                .unwrap_or("http://localhost:11434");
            let expected_dim = match env.get("OLLAMA_EMBEDDING_DIM").filter(|v| !v.is_empty()) {
                None => None,
                Some(raw) => {
                    let n = js::number(Some(raw));
                    if !n.is_finite() || n <= 0.0 {
                        anyhow::bail!("OLLAMA_EMBEDDING_DIM='{raw}' is not a positive number.");
                    }
                    Some(n as usize)
                }
            };
            let num_ctx = env
                .get("OLLAMA_NUM_CTX")
                .filter(|v| !v.is_empty())
                .map(|v| js::number(Some(v)))
                .filter(|n| n.is_finite() && *n > 0.0)
                .map(|n| n as u64);
            Ok(Arc::new(
                OllamaEmbedder::new(
                    Some(url),
                    Some(&cfg.model),
                    expected_dim,
                    num_ctx,
                    cfg.preset_name,
                )
                .with_env(env.clone()),
            ))
        }
        EmbeddingProvider::Transformers => Ok(Arc::new(TransformersEmbedder::new(
            Some(&cfg.model),
            TransformersEmbedderOptions {
                cache_dir: Some(model_cache_dir_from(data_dir, env)),
                ..TransformersEmbedderOptions::default()
            },
        ))),
    }
}

/// One embedder for several vaults. `init` loads the model once, however
/// many vaults ask; a failed init stays failed for all of them.
#[derive(Clone)]
pub struct SharedEmbedder {
    pub embedder: Arc<dyn Embedder>,
    init: Arc<tokio::sync::OnceCell<Result<(), Arc<anyhow::Error>>>>,
}

impl SharedEmbedder {
    pub async fn init(&self) -> Result<(), Arc<anyhow::Error>> {
        self.init
            .get_or_init(|| async { self.embedder.init().await.map_err(Arc::new) })
            .await
            .clone()
    }
}

pub fn share_embedder(embedder: Arc<dyn Embedder>) -> SharedEmbedder {
    SharedEmbedder {
        embedder,
        init: Arc::new(tokio::sync::OnceCell::new()),
    }
}
