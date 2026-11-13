//! Fake embedders, temp vaults and log capture for the core tests.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use obsidian_brain::config::VaultConfig;
use obsidian_brain::context::VaultContext;
use obsidian_brain::embeddings::{
    Embedder, EmbedderMetadata, PrefixSource, TaskType, share_embedder,
};
use obsidian_brain::store::Db;
use tokio::sync::watch;

type EmbedFn = Box<dyn Fn(usize, &str) -> anyhow::Result<Vec<f32>> + Send + Sync>;

/// An embedder that loads nothing. What `embed` returns is up to the test;
/// `init` waits for [`FakeEmbedder::release_init`] when the embedder was
/// built gated.
pub struct FakeEmbedder {
    pub model: String,
    pub dim: usize,
    pub provider: &'static str,
    embed_fn: EmbedFn,
    calls: AtomicUsize,
    pub init_calls: AtomicUsize,
    pub dispose_calls: AtomicUsize,
    gate: Option<watch::Sender<Option<Result<(), String>>>>,
    metadata: Mutex<Option<EmbedderMetadata>>,
}

impl FakeEmbedder {
    fn build(model: &str, dim: usize, provider: &'static str, embed_fn: EmbedFn) -> Self {
        Self {
            model: model.into(),
            dim,
            provider,
            embed_fn,
            calls: AtomicUsize::new(0),
            init_calls: AtomicUsize::new(0),
            dispose_calls: AtomicUsize::new(0),
            gate: None,
            metadata: Mutex::new(None),
        }
    }

    /// Zero vectors of dim 384, as the TS `InstantMockEmbedder`.
    pub fn instant() -> Self {
        Self::build(
            "mock/instant",
            384,
            "mock",
            Box::new(|_, _| Ok(vec![0.0; 384])),
        )
    }

    /// A fixed identity for the bootstrap tests.
    pub fn stub(model: &str, dim: usize, provider: &'static str) -> Self {
        Self::build(
            model,
            dim,
            provider,
            Box::new(move |_, _| Ok(vec![0.0; dim])),
        )
    }

    /// Hashed bag of words: texts that share words land close together, so
    /// semantic search ranks by word overlap.
    pub fn bag_of_words() -> Self {
        Self::build(
            "mock/bag-of-words",
            1024,
            "mock",
            Box::new(|_, t| Ok(bag_of_words(t, 1024))),
        )
    }

    /// Fails the embed call with index `fail_on` (counting every call).
    pub fn faulty_on_call(fail_on: usize, message: &str) -> Self {
        let message = message.to_string();
        Self::build(
            "mock/instant",
            384,
            "mock",
            Box::new(move |i, _| {
                if i == fail_on {
                    anyhow::bail!("{message}")
                }
                Ok(vec![0.0; 384])
            }),
        )
    }

    pub fn always_fail(message: &str) -> Self {
        let message = message.to_string();
        Self::build(
            "mock/instant",
            384,
            "mock",
            Box::new(move |_, _| anyhow::bail!("{message}")),
        )
    }

    /// Fails every text that is or starts with `stem` and a newline: the
    /// note-level text of a note titled `stem`.
    pub fn note_embed_fail(stem: &str, message: &str) -> Self {
        let (stem, message) = (stem.to_string(), message.to_string());
        Self::build(
            "mock/instant",
            384,
            "mock",
            Box::new(move |_, t| {
                if t == stem || t.starts_with(&format!("{stem}\n")) {
                    anyhow::bail!("{message}")
                }
                Ok(vec![0.0; 384])
            }),
        )
    }

    /// `init` waits until [`Self::release_init`].
    pub fn gated(mut self) -> Self {
        self.gate = Some(watch::channel(None).0);
        self
    }

    pub fn release_init(&self, outcome: Result<(), &str>) {
        if let Some(g) = &self.gate {
            g.send_replace(Some(outcome.map_err(str::to_string)));
        }
    }

    pub fn with_prefixes(self, query: &str, document: &str) -> Self {
        *self.metadata.lock().unwrap() = Some(EmbedderMetadata {
            model_id: self.model.clone(),
            dim: Some(self.dim),
            max_tokens: 512,
            query_prefix: query.into(),
            document_prefix: document.into(),
            prefix_source: PrefixSource::Seed,
            base_model: None,
            size_bytes: None,
        });
        self
    }
}

pub fn bag_of_words(text: &str, dim: usize) -> Vec<f32> {
    const STOP: &[&str] = &[
        "a", "an", "and", "the", "of", "with", "to", "in", "for", "used",
    ];
    let mut v = vec![0.0f32; dim];
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| !w.is_empty() && !STOP.contains(&w.as_str()))
    {
        let h = word
            .bytes()
            .fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
        v[(h as usize) % dim] += 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    } else {
        v[0] = 1.0;
    }
    v
}

#[async_trait]
impl Embedder for FakeEmbedder {
    async fn init(&self) -> anyhow::Result<()> {
        self.init_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(g) = &self.gate {
            let mut rx = g.subscribe();
            let state = rx.wait_for(Option::is_some).await?.clone();
            return state.unwrap().map_err(|e| anyhow::anyhow!(e));
        }
        Ok(())
    }
    async fn embed(&self, text: &str, _task: TaskType) -> anyhow::Result<Vec<f32>> {
        let i = self.calls.fetch_add(1, Ordering::SeqCst);
        (self.embed_fn)(i, text)
    }
    fn dimensions(&self) -> anyhow::Result<usize> {
        Ok(self.dim)
    }
    fn model_identifier(&self) -> String {
        self.model.clone()
    }
    fn provider_name(&self) -> &'static str {
        self.provider
    }
    fn set_metadata(&self, meta: EmbedderMetadata) -> anyhow::Result<()> {
        *self.metadata.lock().unwrap() = Some(meta);
        Ok(())
    }
    fn get_metadata(&self) -> Option<EmbedderMetadata> {
        self.metadata.lock().unwrap().clone()
    }
    async fn dispose(&self) {
        self.dispose_calls.fetch_add(1, Ordering::SeqCst);
    }
}

pub fn fixture_vault() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures/vault")
}

pub fn copy_dir(from: &Path, to: &Path) {
    for entry in walkdir::WalkDir::new(from) {
        let entry = entry.unwrap();
        let rel = entry.path().strip_prefix(from).unwrap();
        let dest = to.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&dest).unwrap();
        } else {
            std::fs::copy(entry.path(), &dest).unwrap();
        }
    }
}

pub fn write(vault: &Path, rel: &str, content: &str) {
    let path = vault.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// A note of `n` sections, each long enough to make one chunk.
pub fn multi_section_note(n: usize) -> String {
    (1..=n)
        .map(|i| {
            format!(
                "# Section {i}\n\nThis section has enough content to pass the minimum chunk character threshold.\n"
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// An in-memory index with the vec tables at the fake embedders' dim.
pub fn memory_db() -> Db {
    Db::open_in_memory().unwrap()
}

/// A context over a temp vault and an in-memory index.
pub fn context(vault: &Path, embedder: Arc<dyn Embedder>) -> Arc<VaultContext> {
    let config = VaultConfig::new("test", vault, &vault.join(".data"));
    VaultContext::from_parts(config, memory_db(), share_embedder(embedder), true)
}

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Logs written on this thread while the guard lives.
pub struct LogCapture {
    buf: Buf,
    _guard: tracing::subscriber::DefaultGuard,
}

impl LogCapture {
    pub fn start() -> Self {
        let buf = Buf::default();
        let writer = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        Self {
            buf,
            _guard: tracing::subscriber::set_default(subscriber),
        }
    }

    pub fn text(&self) -> String {
        String::from_utf8(self.buf.0.lock().unwrap().clone()).unwrap()
    }
}
