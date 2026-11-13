//! Ported from `embedder.test.ts`, `embedder-idle-unload.test.ts`,
//! `embedder-prefix.test.ts` and `embedder-preflight.test.ts`. A fake
//! loader stands in for the transformers.js pipeline mock; the tests on the
//! real model download it and are ignored by default.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use obsidian_brain::embeddings::embedder::{
    LoadedModel, ModelLoader, TransformersEmbedder, TransformersEmbedderOptions,
};
use obsidian_brain::embeddings::{
    Embedder, EmbedderLoadError, EmbedderLoadErrorKind, EmbedderMetadata, PrefixSource, TaskType,
};

const IDLE_MS: u64 = 60_000;

/// A latch the fake model waits on; open by default.
#[derive(Default)]
struct Gate {
    closed: Mutex<bool>,
    cv: Condvar,
}

impl Gate {
    fn set(&self, closed: bool) {
        *self.closed.lock().unwrap() = closed;
        self.cv.notify_all();
    }
    fn wait(&self) {
        let mut closed = self.closed.lock().unwrap();
        while *closed {
            closed = self.cv.wait(closed).unwrap();
        }
    }
}

#[derive(Default)]
struct Fake {
    loads: AtomicUsize,
    drops: Arc<AtomicUsize>,
    /// While closed, every embed blocks.
    gate: Arc<Gate>,
    seen: Arc<Mutex<Vec<String>>>,
    fail_with: Option<String>,
}

struct FakeModel {
    drops: Arc<AtomicUsize>,
    gate: Arc<Gate>,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Drop for FakeModel {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

impl LoadedModel for FakeModel {
    fn embed(&mut self, text: &str) -> anyhow::Result<Vec<f32>> {
        self.gate.wait();
        self.seen.lock().unwrap().push(text.to_string());
        Ok(vec![text.encode_utf16().count() as f32, 1.0, 2.0])
    }
    fn model_max_length(&self) -> f64 {
        512.0
    }
}

#[async_trait]
impl ModelLoader for Fake {
    async fn load(&self, _model_id: &str) -> anyhow::Result<Box<dyn LoadedModel>> {
        if let Some(msg) = &self.fail_with {
            anyhow::bail!("{msg}");
        }
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeModel {
            drops: self.drops.clone(),
            gate: self.gate.clone(),
            seen: self.seen.clone(),
        }))
    }
}

fn fake_embedder(idle: u64) -> (TransformersEmbedder, Arc<Fake>) {
    fake_with(idle, Fake::default())
}

fn fake_with(idle: u64, fake: Fake) -> (TransformersEmbedder, Arc<Fake>) {
    let fake = Arc::new(fake);
    let e = TransformersEmbedder::new(
        Some("test/model"),
        TransformersEmbedderOptions {
            idle_unload_ms: Some(idle),
            cache_dir: None,
            loader: Some(fake.clone()),
        },
    );
    (e, fake)
}

/// Move the paused clock and let woken tasks run.
async fn advance(ms: u64) {
    tokio::time::advance(Duration::from_millis(ms)).await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn keeps_the_model_without_an_idle_time() {
    let (e, fake) = fake_embedder(0);
    e.init().await.unwrap();
    advance(24 * 60 * 60 * 1000).await;
    assert!(e.is_loaded());
    assert_eq!(fake.drops.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn unloads_after_idle_and_reloads_on_the_next_embed() {
    let (e, fake) = fake_embedder(IDLE_MS);
    e.init().await.unwrap();
    assert_eq!(fake.loads.load(Ordering::SeqCst), 1);
    advance(IDLE_MS).await;
    assert!(!e.is_loaded());
    assert_eq!(fake.drops.load(Ordering::SeqCst), 1);

    let v = e.embed("hello", TaskType::Document).await.unwrap();
    assert_eq!(v, vec![5.0, 1.0, 2.0]);
    assert_eq!(fake.loads.load(Ordering::SeqCst), 2);
    assert!(e.is_loaded());
}

#[tokio::test(start_paused = true)]
async fn every_embed_restarts_the_idle_clock() {
    let (e, _fake) = fake_embedder(IDLE_MS);
    e.init().await.unwrap();
    advance(IDLE_MS - 1_000).await;
    e.embed("keep me", TaskType::Document).await.unwrap();
    advance(IDLE_MS - 1_000).await;
    assert!(e.is_loaded());
    advance(1_000).await;
    assert!(!e.is_loaded());
}

#[tokio::test(start_paused = true)]
async fn never_unloads_under_a_running_embed() {
    let (e, fake) = fake_embedder(IDLE_MS);
    let e = Arc::new(e);
    e.init().await.unwrap();
    fake.gate.set(true);
    let slow = tokio::spawn({
        let e = e.clone();
        async move { e.embed("slow", TaskType::Document).await }
    });
    advance(IDLE_MS * 3).await;
    assert!(e.is_loaded());
    fake.gate.set(false);
    slow.await.unwrap().unwrap();
    advance(IDLE_MS).await;
    assert!(!e.is_loaded());
    assert_eq!(fake.loads.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn reports_the_tokenizer_limit_while_unloaded() {
    let (e, _fake) = fake_embedder(IDLE_MS);
    e.init().await.unwrap();
    advance(IDLE_MS).await;
    assert!(!e.is_loaded());
    assert_eq!(e.model_max_length(), Some(512.0));
    assert_eq!(e.dimensions().unwrap(), 3);
}

#[tokio::test(start_paused = true)]
async fn dispose_cancels_a_pending_unload() {
    let (e, fake) = fake_embedder(IDLE_MS);
    e.init().await.unwrap();
    e.dispose().await;
    assert_eq!(fake.drops.load(Ordering::SeqCst), 1);
    advance(IDLE_MS).await;
    assert_eq!(fake.drops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn refuses_to_embed_before_init() {
    let (e, _fake) = fake_embedder(IDLE_MS);
    let err = e.embed("x", TaskType::Document).await.unwrap_err();
    assert!(err.to_string().contains("not initialized"));
    assert!(e.dimensions().is_err());
}

#[tokio::test]
async fn concurrent_embeds_both_resolve() {
    let (e, _fake) = fake_embedder(0);
    e.init().await.unwrap();
    let (a, b) = tokio::join!(
        e.embed("concurrent call one", TaskType::Document),
        e.embed("concurrent call two", TaskType::Document)
    );
    assert_eq!(a.unwrap()[0], 19.0);
    assert_eq!(b.unwrap()[0], 19.0);
}

fn bge_meta(model_id: &str) -> EmbedderMetadata {
    EmbedderMetadata {
        model_id: model_id.into(),
        dim: Some(384),
        max_tokens: 512,
        query_prefix: "Represent this sentence for searching relevant passages: ".into(),
        document_prefix: String::new(),
        prefix_source: PrefixSource::Seed,
        base_model: None,
        size_bytes: None,
    }
}

#[tokio::test]
async fn embed_applies_the_metadata_prefix() {
    let (e, fake) = fake_embedder(0);
    assert_eq!(e.get_metadata(), None);
    e.init().await.unwrap();
    e.set_metadata(bge_meta("test/model")).unwrap();
    assert_eq!(e.get_metadata(), Some(bge_meta("test/model")));
    e.embed("cats", TaskType::Query).await.unwrap();
    e.embed("dogs", TaskType::Document).await.unwrap();
    e.set_metadata(EmbedderMetadata {
        query_prefix: "Q {text} / {text}".into(),
        ..bge_meta("test/model")
    })
    .unwrap();
    e.embed("x", TaskType::Query).await.unwrap();
    let seen = fake.seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![
            " ".to_string(),
            "Represent this sentence for searching relevant passages: cats".into(),
            "dogs".into(),
            "Q x / x".into(),
        ]
    );
}

#[test]
fn set_metadata_rejects_another_model() {
    let (e, _fake) = fake_embedder(0);
    let err = e
        .set_metadata(bge_meta("BAAI/bge-base-en-v1.5"))
        .unwrap_err();
    assert!(err.to_string().to_lowercase().contains("model mismatch"));
}

async fn init_error(msg: &str) -> EmbedderLoadError {
    let (e, _fake) = fake_with(
        0,
        Fake {
            fail_with: Some(msg.into()),
            ..Fake::default()
        },
    );
    e.init()
        .await
        .unwrap_err()
        .downcast::<EmbedderLoadError>()
        .unwrap()
}

#[tokio::test]
async fn init_classifies_load_errors() {
    let e = init_error("404 not found: MyOrg/my-model").await;
    assert_eq!(e.kind, EmbedderLoadErrorKind::NotFound);
    assert!(e.message.contains("test/model"));
    let e = init_error("Could not locate file: onnx/model_quantized.onnx").await;
    assert_eq!(e.kind, EmbedderLoadErrorKind::NoOnnx);
    let e = init_error("getaddrinfo ENOTFOUND huggingface.co").await;
    assert_eq!(e.kind, EmbedderLoadErrorKind::Offline);
    let e = init_error("something odd").await;
    assert_eq!(e.kind, EmbedderLoadErrorKind::Unknown);
    assert_eq!(e.cause.as_deref(), Some("something odd"));
}

#[test]
fn builds_the_note_embedding_text() {
    let text = TransformersEmbedder::build_embedding_text(
        "Widget Theory",
        &["concept".into(), "framework".into()],
        "A theoretical framework for understanding component interactions.\n\nMore details here.",
    );
    assert!(text.contains("Widget Theory"));
    assert!(text.contains("concept"));
    assert!(text.contains("theoretical framework"));
    assert!(!text.contains("More details here"));
    let text = TransformersEmbedder::build_embedding_text("Just Title", &[], "Body paragraph.");
    assert_eq!(text, "Just Title\nBody paragraph.");
}

// --- the real model --------------------------------------------------------

fn cache_dir() -> PathBuf {
    std::env::var_os("TRANSFORMERS_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("hf-cache"))
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (na * nb)
}

#[tokio::test]
#[ignore = "downloads Xenova/all-MiniLM-L6-v2 from Hugging Face"]
async fn real_model_embeds() {
    let e = TransformersEmbedder::new(
        None,
        TransformersEmbedderOptions {
            idle_unload_ms: Some(0),
            cache_dir: Some(cache_dir()),
            loader: None,
        },
    );
    e.init().await.unwrap();
    assert_eq!(e.model_identifier(), "Xenova/all-MiniLM-L6-v2");
    assert_eq!(e.model_max_length(), Some(512.0));
    let v = e.embed("Hello world", TaskType::Document).await.unwrap();
    assert_eq!(v.len(), 384);

    let a = e
        .embed("knowledge graph traversal", TaskType::Document)
        .await
        .unwrap();
    let b = e
        .embed("graph traversal in knowledge bases", TaskType::Document)
        .await
        .unwrap();
    let c = e
        .embed("chocolate cake recipe", TaskType::Document)
        .await
        .unwrap();
    assert!(cosine(&a, &b) > cosine(&a, &c));

    let (x, y) = tokio::join!(
        e.embed("concurrent call one", TaskType::Document),
        e.embed("concurrent call two", TaskType::Document)
    );
    for v in [x.unwrap(), y.unwrap()] {
        assert_eq!(v.len(), 384);
        assert!(v.iter().any(|f| *f != 0.0));
    }
    e.dispose().await;
}

#[tokio::test]
#[ignore = "downloads MongoDB/mdbr-leaf-ir from Hugging Face"]
async fn mdbr_leaf_ir_loads_external_data_and_mean_pools() {
    // transformers.js mean-pools `last_hidden_state` and ignores the
    // Dense-projected `sentence_embedding` output, so the vector is 384-d.
    let e = TransformersEmbedder::new(
        Some("MongoDB/mdbr-leaf-ir"),
        TransformersEmbedderOptions {
            idle_unload_ms: Some(0),
            cache_dir: Some(cache_dir()),
            loader: None,
        },
    );
    e.init().await.unwrap();
    assert_eq!(e.dimensions().unwrap(), 384);
    let v = e.embed("hello world", TaskType::Document).await.unwrap();
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-5);
    // The first components from transformers.js 4.3.1, one text per call.
    let want = [-0.022958647, -0.040_180_6, 0.009_005_937, -0.032919724];
    for (got, want) in v.iter().zip(want) {
        assert!((got - want).abs() < 2e-3, "{got} vs {want}");
    }
}
