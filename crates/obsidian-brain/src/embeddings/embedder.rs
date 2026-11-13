//! The local embedder: a sentence-embedding model from Hugging Face, run
//! with ONNX Runtime. It reproduces the transformers.js `feature-extraction`
//! pipeline with `dtype: 'q8'`, mean pooling and L2 normalisation, so it
//! yields the same vectors from the same files.
//!
//! It loads `onnx/model_quantized.onnx` (and its external data files, when
//! `config.json` declares them), `tokenizer.json`, and
//! `tokenizer_config.json` for `model_max_length`. Inputs are truncated to
//! `model_max_length` the way transformers.js truncates: the id list is cut,
//! so a long text loses its final `[SEP]`. `token_type_ids` are fed when the
//! graph takes them. The output is `last_hidden_state` (else `logits`, else
//! `token_embeddings`), mean-pooled over the attention mask. A graph's other
//! outputs, such as the Dense-projected `sentence_embedding` of
//! MongoDB/mdbr-leaf-ir, are ignored, as transformers.js ignored them.
//!
//! Calls run one at a time. With an idle time set, the session is dropped
//! after that long without an embed and loaded again on the next one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use ort::session::Session;
use ort::session::builder::SessionBuilder;
use ort::value::Tensor;
use serde_json::Value;
use tokenizers::pipeline::{EncodeOptions, Override, PipelineTokenizer};

use super::errors::classify_load_error;
use super::types::{Embedder, EmbedderMetadata, TaskType};
use crate::util::js;

pub const DEFAULT_MODEL: &str = "Xenova/all-MiniLM-L6-v2";

/// ONNX Runtime intra-op threads. The host is an 8 GB Mac mini shared with
/// other services: two threads keep an embed fast without taking the CPU.
const INTRA_THREADS: usize = 2;

/// One loaded model: it turns a text into a pooled, normalised vector.
pub trait LoadedModel: Send {
    fn embed(&mut self, text: &str) -> anyhow::Result<Vec<f32>>;
    /// The tokenizer's `model_max_length`; `f64::INFINITY` when unset.
    fn model_max_length(&self) -> f64;
}

/// Loads a model by id. The ONNX loader downloads from Hugging Face; tests
/// substitute a fake.
#[async_trait]
pub trait ModelLoader: Send + Sync {
    async fn load(&self, model_id: &str) -> anyhow::Result<Box<dyn LoadedModel>>;
}

/// `OBSIDIAN_BRAIN_EMBEDDER_IDLE_MS`; 0, unset or junk keeps the model.
fn read_idle_unload_ms(env: &HashMap<String, String>) -> u64 {
    let raw = js::number(
        env.get("OBSIDIAN_BRAIN_EMBEDDER_IDLE_MS")
            .map(String::as_str),
    );
    if raw.is_finite() && raw > 0.0 {
        raw as u64
    } else {
        0
    }
}

#[derive(Default)]
pub struct TransformersEmbedderOptions {
    /// Unload the model after this many ms without an embed. 0 keeps it.
    /// Defaults to `OBSIDIAN_BRAIN_EMBEDDER_IDLE_MS`.
    pub idle_unload_ms: Option<u64>,
    /// Where model files are cached. Defaults to `TRANSFORMERS_CACHE`, else
    /// the hf-hub default.
    pub cache_dir: Option<PathBuf>,
    /// Replaces the ONNX loader.
    pub loader: Option<Arc<dyn ModelLoader>>,
}

struct Inner {
    model: String,
    loader: Arc<dyn ModelLoader>,
    idle_unload_ms: u64,
    /// The loaded model. The lock is the embed queue: it is fair, so calls
    /// run in arrival order.
    slot: tokio::sync::Mutex<Option<Box<dyn LoadedModel>>>,
    loaded: AtomicBool,
    dim: Mutex<Option<usize>>,
    metadata: Mutex<Option<EmbedderMetadata>>,
    model_max_length: Mutex<Option<f64>>,
    /// Embeds queued or running; the model never unloads under them.
    pending: AtomicUsize,
    idle_timer: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// The ONNX embedder. Cheap to share: the model loads once.
pub struct TransformersEmbedder {
    inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl TransformersEmbedder {
    /// `model` defaults to `EMBEDDING_MODEL`, else `DEFAULT_MODEL`.
    pub fn new(model: Option<&str>, opts: TransformersEmbedderOptions) -> Self {
        let env = js::process_env();
        let model = model
            .map(str::to_string)
            .or_else(|| env.get("EMBEDDING_MODEL").cloned())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let loader = opts.loader.unwrap_or_else(|| {
            let cache_dir = opts
                .cache_dir
                .or_else(|| js::env_value(&env, "TRANSFORMERS_CACHE").map(PathBuf::from));
            Arc::new(OnnxLoader { cache_dir })
        });
        Self {
            inner: Arc::new(Inner {
                model,
                loader,
                idle_unload_ms: opts
                    .idle_unload_ms
                    .unwrap_or_else(|| read_idle_unload_ms(&env)),
                slot: tokio::sync::Mutex::new(None),
                loaded: AtomicBool::new(false),
                dim: Mutex::new(None),
                metadata: Mutex::new(None),
                model_max_length: Mutex::new(None),
                pending: AtomicUsize::new(0),
                idle_timer: Mutex::new(None),
            }),
        }
    }

    /// Whether the model is in memory now.
    pub fn is_loaded(&self) -> bool {
        self.inner.loaded.load(Ordering::SeqCst)
    }

    /// The title, the tags and the first paragraph, joined by newlines: the
    /// text of the note-level embedding.
    pub fn build_embedding_text(title: &str, tags: &[String], content: &str) -> String {
        let first_paragraph = PARAGRAPH_BREAK.split(content).next().unwrap_or_default();
        let mut parts = vec![title.to_string()];
        if !tags.is_empty() {
            parts.push(tags.join(", "));
        }
        if !first_paragraph.is_empty() {
            parts.push(first_paragraph.to_string());
        }
        parts.join("\n")
    }
}

static PARAGRAPH_BREAK: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"\n\n+").unwrap());

impl Inner {
    async fn load(&self) -> anyhow::Result<Box<dyn LoadedModel>> {
        let model = self.loader.load(&self.model).await?;
        *lock(&self.model_max_length) = Some(model.model_max_length());
        Ok(model)
    }

    fn cancel_idle_unload(&self) {
        if let Some(handle) = lock(&self.idle_timer).take() {
            handle.abort();
        }
    }

    fn schedule_idle_unload(self: &Arc<Self>) {
        if self.idle_unload_ms == 0 {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        self.cancel_idle_unload();
        let me = Arc::downgrade(self);
        // The deadline counts from now, not from when the task first runs.
        let deadline = tokio::time::Instant::now() + Duration::from_millis(self.idle_unload_ms);
        let handle = runtime.spawn(async move {
            tokio::time::sleep_until(deadline).await;
            if let Some(me) = me.upgrade() {
                me.unload_if_idle().await;
            }
        });
        *lock(&self.idle_timer) = Some(handle);
    }

    /// Waits its turn in the embed queue, so it never overlaps an embed. An
    /// embed that arrives meanwhile raises `pending`, and the model stays.
    async fn unload_if_idle(&self) {
        let mut slot = self.slot.lock().await;
        if self.pending.load(Ordering::SeqCst) > 0 || slot.is_none() {
            return;
        }
        drop(slot.take());
        self.loaded.store(false, Ordering::SeqCst);
        let (model, ms) = (&self.model, self.idle_unload_ms);
        tracing::info!(
            model = %model,
            idle_unload_ms = ms,
            "unloaded embedding model {model} after {ms} ms idle"
        );
    }

    /// Run one text through the model, loading it first when unloaded.
    async fn run(&self, text: String, reload_log: bool) -> anyhow::Result<Vec<f32>> {
        let mut slot = self.slot.lock().await;
        let model = match slot.take() {
            Some(m) => m,
            None => {
                let m = self.load().await?;
                if reload_log {
                    let model = &self.model;
                    tracing::info!(model = %model, "reloaded embedding model {model}");
                }
                m
            }
        };
        self.loaded.store(true, Ordering::SeqCst);
        let (model, result) = tokio::task::spawn_blocking(move || {
            let mut model = model;
            let result = model.embed(&text);
            (model, result)
        })
        .await
        .map_err(|e| {
            self.loaded.store(false, Ordering::SeqCst);
            anyhow!("embedding task failed: {e}")
        })?;
        *slot = Some(model);
        result
    }
}

/// Lowers `pending` when an embed ends, on every path.
struct PendingGuard<'a>(&'a Arc<Inner>);

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        if self.0.pending.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.schedule_idle_unload();
        }
    }
}

#[async_trait]
impl Embedder for TransformersEmbedder {
    async fn init(&self) -> anyhow::Result<()> {
        let inner = &self.inner;
        let probe = inner.run(" ".to_string(), false).await;
        let result = probe.and_then(|vec| {
            // Probe the output length, so callers can check the vec0 dim
            // before any embed is written.
            if vec.is_empty() {
                anyhow::bail!(
                    "Embedder produced empty vector for model \"{}\". Check the model exists on Hugging Face and outputs sentence embeddings.",
                    inner.model
                );
            }
            Ok(vec.len())
        });
        match result {
            Ok(dim) => {
                *lock(&inner.dim) = Some(dim);
                inner.schedule_idle_unload();
                Ok(())
            }
            Err(err) => Err(classify_load_error(&inner.model, err).into()),
        }
    }

    async fn embed(&self, text: &str, task: TaskType) -> anyhow::Result<Vec<f32>> {
        let inner = &self.inner;
        if lock(&inner.dim).is_none() {
            anyhow::bail!("Embedder not initialized. Call init() first.");
        }
        inner.cancel_idle_unload();
        inner.pending.fetch_add(1, Ordering::SeqCst);
        let _guard = PendingGuard(inner);
        // Without metadata the prefix is empty: symmetric, degraded, but
        // working.
        let prefix = lock(&inner.metadata)
            .as_ref()
            .map(|m| match task {
                TaskType::Query => m.query_prefix.clone(),
                TaskType::Document => m.document_prefix.clone(),
            })
            .unwrap_or_default();
        // Seed templates use only `{text}` placeholders, possibly several.
        let prefixed = if prefix.contains("{text}") {
            prefix.replace("{text}", text)
        } else {
            format!("{prefix}{text}")
        };
        inner.run(prefixed, true).await
    }

    fn dimensions(&self) -> anyhow::Result<usize> {
        lock(&self.inner.dim).ok_or_else(|| anyhow!("Embedder not initialized. Call init() first."))
    }

    fn model_identifier(&self) -> String {
        self.inner.model.clone()
    }

    fn provider_name(&self) -> &'static str {
        "transformers.js"
    }

    /// Call after `init` and before the first embed, so the prefix applies.
    fn set_metadata(&self, meta: EmbedderMetadata) -> anyhow::Result<()> {
        if meta.model_id != self.inner.model {
            anyhow::bail!(
                "obsidian-brain: TransformersEmbedder.setMetadata: model mismatch — metadata is for {}, embedder is for {}",
                meta.model_id,
                self.inner.model
            );
        }
        *lock(&self.inner.metadata) = Some(meta);
        Ok(())
    }

    fn get_metadata(&self) -> Option<EmbedderMetadata> {
        lock(&self.inner.metadata).clone()
    }

    /// Recorded at load, so it stays readable while the model is unloaded.
    fn model_max_length(&self) -> Option<f64> {
        *lock(&self.inner.model_max_length)
    }

    async fn dispose(&self) {
        self.inner.cancel_idle_unload();
        let mut slot = self.inner.slot.lock().await;
        drop(slot.take());
        self.inner.loaded.store(false, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// ONNX Runtime
// ---------------------------------------------------------------------------

/// The ONNX Runtime environment, created once per process. `ORT_DYLIB_PATH`
/// names the shared library; the Nix wrapper sets it.
fn ort_environment() -> anyhow::Result<&'static ort::environment::Environment> {
    static ENV: OnceLock<Result<ort::environment::Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| {
        let builder = match std::env::var_os("ORT_DYLIB_PATH") {
            Some(path) => ort::init_from(path).map_err(|e| e.to_string())?,
            None => ort::init(),
        };
        builder
            .with_name("obsidian-brain")
            .build()
            .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| anyhow!("could not start ONNX Runtime: {e}"))
}

fn ort_err(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("{e}")
}

/// Session settings for a small, shared host. The CPU arena and the memory
/// pattern both keep freed buffers for reuse; with variable input lengths
/// they mostly hold memory, so both are off. Threads do not spin between
/// runs, so an idle server costs no CPU.
fn session_builder() -> anyhow::Result<SessionBuilder> {
    let env = ort_environment()?;
    Session::builder(env)
        .map_err(ort_err)?
        .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])
        .map_err(ort_err)?
        .with_memory_pattern(false)
        .map_err(ort_err)?
        .with_intra_threads(INTRA_THREADS)
        .map_err(ort_err)?
        .with_inter_threads(1)
        .map_err(ort_err)?
        .with_parallel_execution(false)
        .map_err(ort_err)?
        .with_config_entry("session.intra_op.allow_spinning", "0")
        .map_err(ort_err)?
        .with_config_entry("session.inter_op.allow_spinning", "0")
        .map_err(ort_err)
}

/// The cache layout and the repo files that transformers.js would fetch.
pub struct OnnxLoader {
    cache_dir: Option<PathBuf>,
}

impl OnnxLoader {
    pub fn new(cache_dir: Option<PathBuf>) -> Self {
        Self { cache_dir }
    }

    fn api(&self) -> anyhow::Result<hf_hub::api::tokio::Api> {
        let mut builder = hf_hub::api::tokio::ApiBuilder::from_env().with_progress(false);
        if let Some(dir) = &self.cache_dir {
            builder = builder.with_cache_dir(dir.clone());
        }
        Ok(builder.build()?)
    }

    /// The cache subtree of one model, for the corrupt-file recovery.
    fn model_cache_dir(&self, model_id: &str) -> Option<PathBuf> {
        let root = self
            .cache_dir
            .clone()
            .unwrap_or_else(|| hf_hub::Cache::from_env().path().clone());
        let folder = hf_hub::Repo::model(model_id.to_string()).folder_name();
        Some(root.join(folder))
    }

    async fn fetch_files(&self, model_id: &str) -> anyhow::Result<ModelFiles> {
        let api = self.api()?;
        let repo = api.model(model_id.to_string());
        let config_path = repo.get("config.json").await?;
        let config: Value = serde_json::from_str(&std::fs::read_to_string(&config_path)?)?;
        let tokenizer = repo.get("tokenizer.json").await?;
        let tokenizer_config = repo.get("tokenizer_config.json").await.ok();

        let file_name = "model_quantized.onnx";
        let onnx = repo
            .get(&format!("onnx/{file_name}"))
            .await
            .with_context(|| format!("Could not locate file: onnx/{file_name}"))?;
        // External weights, as `transformers.js_config.use_external_data_format`
        // declares them: `<file>_data`, `<file>_data_1`, ...
        let chunks = external_data_chunks(&config, file_name, "model");
        for i in 0..chunks {
            let suffix = if i == 0 {
                String::new()
            } else {
                format!("_{i}")
            };
            repo.get(&format!("onnx/{file_name}_data{suffix}")).await?;
        }
        Ok(ModelFiles {
            onnx,
            tokenizer,
            tokenizer_config,
        })
    }
}

struct ModelFiles {
    onnx: PathBuf,
    tokenizer: PathBuf,
    tokenizer_config: Option<PathBuf>,
}

/// The external data chunk count for an ONNX file, as transformers.js
/// resolves `use_external_data_format`: a per-file map, or one value for
/// all files.
fn external_data_chunks(config: &Value, full_name: &str, base_name: &str) -> usize {
    let Some(v) = config
        .get("transformers.js_config")
        .and_then(|c| c.get("use_external_data_format"))
    else {
        return 0;
    };
    let count = |v: &Value| match v {
        Value::Bool(b) => usize::from(*b),
        Value::Number(n) => n.as_f64().map(|f| f.max(0.0) as usize).unwrap_or(0),
        _ => 0,
    };
    match v {
        Value::Object(map) => map
            .get(full_name)
            .or_else(|| map.get(base_name))
            .map(count)
            .unwrap_or(0),
        other => count(other),
    }
}

fn is_corrupt(msg: &str) -> bool {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)Protobuf parsing failed|Load model .* failed|Invalid model file")
            .unwrap()
    });
    RE.is_match(msg)
}

#[async_trait]
impl ModelLoader for OnnxLoader {
    /// Download and open the model. A truncated download from a killed run
    /// fails to parse; then the model's cache subtree is deleted and the
    /// load runs once more.
    async fn load(&self, model_id: &str) -> anyhow::Result<Box<dyn LoadedModel>> {
        let files = self.fetch_files(model_id).await?;
        match tokio::task::spawn_blocking(move || OnnxModel::open(files)).await? {
            Ok(m) => Ok(Box::new(m)),
            Err(err) => {
                let msg = format!("{err:#}");
                if !is_corrupt(&msg) {
                    return Err(err);
                }
                let snippet: String = msg.chars().take(120).collect();
                tracing::warn!(
                    model = model_id,
                    error_snippet = %snippet,
                    "embedder load failed with \"{snippet}...\"; this is usually a corrupt HF cache from a killed prior download. Clearing the cache for \"{model_id}\" and retrying once."
                );
                if let Some(dir) = self.model_cache_dir(model_id)
                    && let Err(e) = std::fs::remove_dir_all(&dir)
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    tracing::warn!(model = model_id, "could not clear HF cache directory: {e}");
                }
                let files = self.fetch_files(model_id).await?;
                let m = tokio::task::spawn_blocking(move || OnnxModel::open(files)).await??;
                Ok(Box::new(m))
            }
        }
    }
}

struct OnnxModel {
    session: Session,
    tokenizer: PipelineTokenizer,
    model_max_length: f64,
    output_name: String,
    has_attention_mask: bool,
    has_token_type_ids: bool,
}

fn read_model_max_length(path: Option<&Path>) -> f64 {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("model_max_length").and_then(Value::as_f64))
        .unwrap_or(f64::INFINITY)
}

fn load_tokenizer(path: &Path) -> anyhow::Result<PipelineTokenizer> {
    // A tokenizer.json from before tokenizers 1.0 needs the upgrade pass.
    let canonical = tokenizers::canonicalize_file(path).map_err(ort_err)?;
    tokenizers::from_json(&canonical).map_err(ort_err)
}

impl OnnxModel {
    fn open(files: ModelFiles) -> anyhow::Result<Self> {
        let tokenizer = load_tokenizer(&files.tokenizer)?;
        let model_max_length = read_model_max_length(files.tokenizer_config.as_deref());
        let session = session_builder()?
            .commit_from_file(&files.onnx)
            .map_err(ort_err)?;
        let inputs: Vec<&str> = session.inputs().iter().map(|i| i.name()).collect();
        let has_attention_mask = inputs.contains(&"attention_mask");
        let has_token_type_ids = inputs.contains(&"token_type_ids");
        let outputs: Vec<&str> = session.outputs().iter().map(|o| o.name()).collect();
        let output_name = ["last_hidden_state", "logits", "token_embeddings"]
            .into_iter()
            .find(|n| outputs.contains(n))
            .ok_or_else(|| anyhow!("model has no last_hidden_state output: {outputs:?}"))?
            .to_string();
        Ok(Self {
            session,
            tokenizer,
            model_max_length,
            output_name,
            has_attention_mask,
            has_token_type_ids,
        })
    }
}

impl LoadedModel for OnnxModel {
    fn embed(&mut self, text: &str) -> anyhow::Result<Vec<f32>> {
        let options = EncodeOptions {
            padding: Override::Off,
            truncation: Override::Off,
            ..EncodeOptions::default()
        };
        let encoding = self
            .tokenizer
            .encode(text, &options)
            .wait()
            .map_err(ort_err)?
            .pop()
            .ok_or_else(|| anyhow!("tokenizer returned no encoding"))?;
        let mut ids: Vec<i64> = encoding.ids().iter().map(|t| i64::from(t.id())).collect();
        let mut types: Vec<i64> = match encoding.type_ids() {
            Some(t) => t.iter().map(|&x| i64::from(x)).collect(),
            None => vec![0; ids.len()],
        };
        // transformers.js truncates by cutting each array to the max length.
        if ids.len() as f64 > self.model_max_length {
            let max = self.model_max_length as usize;
            ids.truncate(max);
            types.truncate(max);
        }
        let n = ids.len();
        let mask = vec![1i64; n];

        let mut inputs: Vec<(String, ort::session::SessionInputValue<'_>)> = vec![(
            "input_ids".into(),
            Tensor::from_array(([1usize, n], ids))
                .map_err(ort_err)?
                .into(),
        )];
        if self.has_attention_mask {
            inputs.push((
                "attention_mask".into(),
                Tensor::from_array(([1usize, n], mask.clone()))
                    .map_err(ort_err)?
                    .into(),
            ));
        }
        if self.has_token_type_ids {
            inputs.push((
                "token_type_ids".into(),
                Tensor::from_array(([1usize, n], types))
                    .map_err(ort_err)?
                    .into(),
            ));
        }
        let outputs = self.session.run(inputs).map_err(ort_err)?;
        let (shape, data) = outputs[self.output_name.as_str()]
            .try_extract_tensor::<f32>()
            .map_err(ort_err)?;
        let dims: Vec<i64> = shape.iter().copied().collect();
        let [_, seq, dim] = dims[..] else {
            anyhow::bail!("unexpected output shape {dims:?}");
        };
        Ok(mean_pool_normalize(data, &mask, seq as usize, dim as usize))
    }

    fn model_max_length(&self) -> f64 {
        self.model_max_length
    }
}

/// transformers.js `mean_pooling` then `normalize(2, -1)`, with the same
/// arithmetic: f64 sums stored into f32, and an f32 accumulator for the
/// norm.
pub fn mean_pool_normalize(hidden: &[f32], mask: &[i64], seq: usize, dim: usize) -> Vec<f32> {
    let mut pooled = vec![0f32; dim];
    for (k, out) in pooled.iter_mut().enumerate() {
        let mut sum = 0f64;
        let mut count = 0f64;
        for j in 0..seq {
            let attn = mask[j] as f64;
            count += attn;
            sum += f64::from(hidden[j * dim + k]) * attn;
        }
        *out = (sum / count) as f32;
    }
    let mut acc = 0f32;
    for &x in &pooled {
        acc = (f64::from(acc) + f64::from(x) * f64::from(x)) as f32;
    }
    let norm = f64::from(acc).sqrt() as f32;
    pooled
        .iter()
        .map(|&x| (f64::from(x) / f64::from(norm)) as f32)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn external_data_chunks_follow_transformers_js() {
        let c = json!({"transformers.js_config": {"use_external_data_format": {"model_quantized.onnx": 1}}});
        assert_eq!(external_data_chunks(&c, "model_quantized.onnx", "model"), 1);
        assert_eq!(
            external_data_chunks(&json!({}), "model_quantized.onnx", "model"),
            0
        );
        let c = json!({"transformers.js_config": {"use_external_data_format": true}});
        assert_eq!(external_data_chunks(&c, "model_quantized.onnx", "model"), 1);
        let c = json!({"transformers.js_config": {"use_external_data_format": {"model": 2}}});
        assert_eq!(external_data_chunks(&c, "model_quantized.onnx", "model"), 2);
    }

    #[test]
    fn mean_pool_and_normalize() {
        // Two tokens of dim 2: mean is (2, 0), normalised (1, 0).
        let v = mean_pool_normalize(&[1.0, 0.0, 3.0, 0.0], &[1, 1], 2, 2);
        assert_eq!(v, vec![1.0, 0.0]);
    }

    #[test]
    fn idle_ms_from_env() {
        let env = |v: &str| HashMap::from([("OBSIDIAN_BRAIN_EMBEDDER_IDLE_MS".into(), v.into())]);
        assert_eq!(read_idle_unload_ms(&env("60000")), 60000);
        assert_eq!(read_idle_unload_ms(&env("0")), 0);
        assert_eq!(read_idle_unload_ms(&env("-5")), 0);
        assert_eq!(read_idle_unload_ms(&env("soon")), 0);
        assert_eq!(read_idle_unload_ms(&HashMap::new()), 0);
    }

    #[test]
    fn builds_embedding_text() {
        let text = TransformersEmbedder::build_embedding_text(
            "Title",
            &["a".into(), "b".into()],
            "First para.\n\nSecond.",
        );
        assert_eq!(text, "Title\na, b\nFirst para.");
        let text = TransformersEmbedder::build_embedding_text("Title", &[], "Body");
        assert_eq!(text, "Title\nBody");
    }
}
