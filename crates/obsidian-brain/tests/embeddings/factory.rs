//! Tests of the embedder factory. The provider name tells the two embedders
//! apart.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use obsidian_brain::embeddings::factory::{create_embedder_with_env, model_cache_dir_from};
use obsidian_brain::embeddings::presets::{
    DEFAULT_OLLAMA_MODEL, EMBEDDING_PRESETS, EmbeddingProvider,
};
use obsidian_brain::embeddings::{Embedder, TaskType, share_embedder};
use serde_json::json;

use crate::mock_http::{MockServer, json};
use crate::support::env;

fn create(pairs: &[(&str, &str)]) -> anyhow::Result<Arc<dyn Embedder>> {
    create_embedder_with_env(Path::new("/data"), &env(pairs))
}

#[test]
fn defaults_to_the_onnx_embedder() {
    let e = create(&[]).unwrap();
    assert_eq!(e.provider_name(), "transformers.js");
    assert_eq!(e.model_identifier(), "Xenova/bge-small-en-v1.5");
    let e = create(&[("EMBEDDING_PROVIDER", "transformers")]).unwrap();
    assert_eq!(e.provider_name(), "transformers.js");
}

#[test]
fn provider_name_is_case_insensitive() {
    let e = create(&[
        ("EMBEDDING_PROVIDER", "OLLAMA"),
        ("OLLAMA_EMBEDDING_DIM", "768"),
    ])
    .unwrap();
    assert_eq!(e.provider_name(), "ollama");
}

#[test]
fn ollama_provider() {
    let e = create(&[
        ("EMBEDDING_PROVIDER", "ollama"),
        ("OLLAMA_EMBEDDING_DIM", "768"),
    ])
    .unwrap();
    assert_eq!(e.provider_name(), "ollama");
    assert_eq!(e.dimensions().unwrap(), 768);
    assert_eq!(e.model_identifier(), "ollama:nomic-embed-text");
}

#[test]
fn ollama_base_url_and_model() {
    let e = create(&[
        ("EMBEDDING_PROVIDER", "ollama"),
        ("OLLAMA_BASE_URL", "http://192.168.1.50:11434"),
        ("EMBEDDING_MODEL", "mxbai-embed-large"),
        ("OLLAMA_EMBEDDING_DIM", "1024"),
    ])
    .unwrap();
    assert_eq!(e.model_identifier(), "ollama:mxbai-embed-large");
    assert_eq!(e.dimensions().unwrap(), 1024);
    assert_eq!(e.base_url().as_deref(), Some("http://192.168.1.50:11434"));
}

#[test]
fn ollama_without_a_dim_errors_until_init() {
    let e = create(&[("EMBEDDING_PROVIDER", "ollama")]).unwrap();
    assert!(
        e.dimensions()
            .unwrap_err()
            .to_string()
            .contains("dimensions not known yet")
    );
}

#[test]
fn unknown_provider_lists_the_valid_ones() {
    let err = create(&[("EMBEDDING_PROVIDER", "openai")])
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("Unknown EMBEDDING_PROVIDER='openai'"));
    assert!(err.contains("transformers") && err.contains("ollama"));
}

#[test]
fn rejects_a_bad_ollama_dim() {
    for dim in ["seven hundred and sixty-eight", "0", "-3"] {
        let err = create(&[
            ("EMBEDDING_PROVIDER", "ollama"),
            ("OLLAMA_EMBEDDING_DIM", dim),
        ])
        .err()
        .unwrap()
        .to_string();
        assert!(
            err.contains("OLLAMA_EMBEDDING_DIM") && err.contains("not a positive number"),
            "{err}"
        );
    }
}

#[test]
fn every_preset_gets_its_model_and_provider() {
    for (name, preset) in EMBEDDING_PRESETS {
        let e = create(&[("EMBEDDING_PRESET", name), ("OLLAMA_EMBEDDING_DIM", "1024")]).unwrap();
        match preset.provider {
            EmbeddingProvider::Ollama => {
                assert_eq!(e.provider_name(), "ollama");
                assert_eq!(e.model_identifier(), format!("ollama:{}", preset.model));
            }
            EmbeddingProvider::Transformers => {
                assert_eq!(e.provider_name(), "transformers.js");
                assert_eq!(e.model_identifier(), preset.model);
            }
        }
    }
}

#[test]
fn ollama_alone_uses_the_default_ollama_model() {
    let e = create(&[
        ("EMBEDDING_PROVIDER", "ollama"),
        ("OLLAMA_EMBEDDING_DIM", "768"),
    ])
    .unwrap();
    assert_eq!(
        e.model_identifier(),
        format!("ollama:{DEFAULT_OLLAMA_MODEL}")
    );
}

#[test]
fn embedding_model_alone_means_transformers() {
    let e = create(&[("EMBEDDING_MODEL", "BAAI/bge-large-en-v1.5")]).unwrap();
    assert_eq!(e.provider_name(), "transformers.js");
    assert_eq!(e.model_identifier(), "BAAI/bge-large-en-v1.5");
}

#[test]
fn embedding_model_on_ollama() {
    let e = create(&[
        ("EMBEDDING_MODEL", "mxbai-embed-large"),
        ("EMBEDDING_PROVIDER", "ollama"),
        ("OLLAMA_EMBEDDING_DIM", "1024"),
    ])
    .unwrap();
    assert_eq!(e.model_identifier(), "ollama:mxbai-embed-large");
}

/// The preset name reaches the Ollama embedder: a preset model is pulled,
/// a third-party model without a preset is refused.
async fn pull_attempted(pairs: &[(&str, &str)]) -> bool {
    let server = MockServer::start(vec![
        json(404, json!({})),
        json(200, json!({"models": []})),
        json(500, json!({})),
    ]);
    let mut pairs = pairs.to_vec();
    pairs.push(("OLLAMA_BASE_URL", &server.url));
    pairs.push(("OLLAMA_EMBEDDING_DIM", "1024"));
    let e = create(&pairs).unwrap();
    assert!(e.init().await.is_err());
    server.called("/api/pull")
}

#[tokio::test]
async fn preset_name_passes_through() {
    assert!(pull_attempted(&[("EMBEDDING_PRESET", "multilingual-ollama")]).await);
    assert!(
        !pull_attempted(&[
            ("EMBEDDING_PROVIDER", "ollama"),
            ("EMBEDDING_MODEL", "user/custom-fork")
        ])
        .await
    );
}

async fn factory_num_ctx(value: &str) -> serde_json::Value {
    let server = MockServer::start(vec![json(200, json!({"embedding": [0.1, 0.2, 0.3]}))]);
    let e = create(&[
        ("EMBEDDING_PROVIDER", "ollama"),
        ("OLLAMA_NUM_CTX", value),
        ("OLLAMA_BASE_URL", &server.url),
    ])
    .unwrap();
    e.embed("test", TaskType::Document).await.unwrap();
    server.requests()[0].json()["options"]["num_ctx"].clone()
}

#[tokio::test]
async fn factory_passes_num_ctx() {
    assert_eq!(factory_num_ctx("4096").await, 4096);
    assert_eq!(factory_num_ctx("").await, 8192);
    assert_eq!(factory_num_ctx("banana").await, 8192);
}

#[test]
fn model_cache_dir_honours_transformers_cache() {
    assert_eq!(
        model_cache_dir_from(Path::new("/data"), &env(&[])),
        PathBuf::from("/data/models")
    );
    assert_eq!(
        model_cache_dir_from(
            Path::new("/data"),
            &env(&[("TRANSFORMERS_CACHE", "/cache")])
        ),
        PathBuf::from("/cache")
    );
}

struct CountingEmbedder(AtomicUsize);

#[async_trait]
impl Embedder for CountingEmbedder {
    async fn init(&self) -> anyhow::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        Ok(())
    }
    async fn embed(&self, _: &str, _: TaskType) -> anyhow::Result<Vec<f32>> {
        Ok(vec![])
    }
    fn dimensions(&self) -> anyhow::Result<usize> {
        Ok(1)
    }
    fn model_identifier(&self) -> String {
        "counting".into()
    }
    fn provider_name(&self) -> &'static str {
        "stub"
    }
    async fn dispose(&self) {}
}

#[tokio::test]
async fn shared_embedder_inits_once() {
    let inner = Arc::new(CountingEmbedder(AtomicUsize::new(0)));
    let shared = share_embedder(inner.clone());
    let other = shared.clone();
    let (a, b) = tokio::join!(shared.init(), other.init());
    assert!(a.is_ok() && b.is_ok());
    shared.init().await.unwrap();
    assert_eq!(inner.0.load(Ordering::SeqCst), 1);
}
