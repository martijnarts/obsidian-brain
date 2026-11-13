//! Shared fixtures: the store tables, stub embedders, captured logs.

use std::io::Write;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use obsidian_brain::embeddings::{Embedder, TaskType};
use rusqlite::Connection;

/// The two tables, exactly as `src/store/db.ts` creates them.
pub fn open_test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS embedder_capability (
           embedder_id TEXT NOT NULL,
           model_hash TEXT NOT NULL,
           advertised_max_tokens INTEGER,
           discovered_max_tokens INTEGER,
           discovered_at INTEGER,
           method TEXT,
           dim INTEGER,
           query_prefix TEXT,
           document_prefix TEXT,
           prefix_source TEXT,
           base_model TEXT,
           size_bytes INTEGER,
           fetched_at INTEGER,
           PRIMARY KEY (embedder_id, model_hash)
         );
         CREATE TABLE IF NOT EXISTS failed_chunks (
           chunk_id TEXT PRIMARY KEY,
           note_id TEXT NOT NULL,
           reason TEXT NOT NULL,
           error_message TEXT,
           failed_at INTEGER NOT NULL
         );",
    )
    .unwrap();
    conn
}

/// An embedder with a fixed identity. `model_max_length` stands in for the
/// loaded tokenizer of the ONNX embedder; `base_url` for Ollama's.
pub struct StubEmbedder {
    pub model: String,
    pub dim: usize,
    pub provider: &'static str,
    pub model_max_length: Option<f64>,
    pub base_url: Option<String>,
}

impl StubEmbedder {
    pub fn new(model: &str, dim: usize) -> Self {
        Self {
            model: model.to_string(),
            dim,
            provider: "stub",
            model_max_length: None,
            base_url: None,
        }
    }

    pub fn transformers(model: &str, max_length: Option<f64>) -> Self {
        Self {
            provider: "transformers.js",
            model_max_length: max_length,
            ..Self::new(model, 384)
        }
    }

    pub fn ollama(model: &str, base_url: &str) -> Self {
        Self {
            provider: "ollama",
            base_url: Some(base_url.to_string()),
            ..Self::new(&format!("ollama:{model}"), 768)
        }
    }
}

#[async_trait]
impl Embedder for StubEmbedder {
    async fn init(&self) -> anyhow::Result<()> {
        Ok(())
    }
    async fn embed(&self, _text: &str, _task: TaskType) -> anyhow::Result<Vec<f32>> {
        Ok(vec![0.0; self.dim])
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
    fn model_max_length(&self) -> Option<f64> {
        self.model_max_length
    }
    fn base_url(&self) -> Option<String> {
        self.base_url.clone()
    }
    async fn dispose(&self) {}
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

/// Run `f` and return what it logged on this thread.
pub fn capture_logs(f: impl FnOnce()) -> String {
    let buf = Buf::default();
    let writer = buf.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    String::from_utf8(buf.0.lock().unwrap().clone()).unwrap()
}

pub fn env(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}
