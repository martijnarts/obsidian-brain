//! An embedder backed by an Ollama server's `/api/embeddings`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use anyhow::anyhow;
use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Value, json};

use super::js;
use super::presets::DEFAULT_OLLAMA_MODEL;
use super::types::{Embedder, EmbedderMetadata, PrefixSource, TaskType};

/// The lifecycle of an Ollama embedder, readable at any time so tools can
/// report pull progress instead of blocking on a long pull.
///
/// not-started → probing → (ready | pulling | failed);
/// pulling → (probing → ready | failed).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum OllamaPhase {
    NotStarted,
    Probing,
    #[serde(rename_all = "camelCase")]
    Pulling {
        /// Progress in MB, rounded down. 0 before the first progress event.
        completed_mb: u64,
        /// Total in MB. 0 until Ollama reports a total.
        total_mb: u64,
        /// 0 to 100, rounded down.
        pct: u64,
        /// The latest Ollama status, such as "pulling manifest".
        phase: String,
    },
    Ready,
    Failed {
        error: String,
    },
}

#[derive(Default)]
struct State {
    cached_dim: Option<usize>,
    cached_digest: Option<String>,
    cached_context_length: Option<u64>,
    /// `/api/show` answered 404: the model is not pulled.
    model_not_pulled: bool,
    /// The last init or embed error, until the next call succeeds.
    /// `dimensions()` returns it, so the cause reaches the client.
    last_error: Option<String>,
    phase: Option<OllamaPhase>,
    metadata: Option<EmbedderMetadata>,
}

/// Talks to Ollama's `/api/embeddings`. The dimension comes from
/// `OLLAMA_EMBEDDING_DIM`, from `/api/show`, or from the first embed.
pub struct OllamaEmbedder {
    base_url: String,
    model: String,
    /// The user's `OLLAMA_NUM_CTX`, if set.
    explicit_num_ctx: Option<u64>,
    /// The preset that chose the model, or `None` when the user named the
    /// model directly.
    preset_name: Option<String>,
    /// The environment the auto-pull switches are read from.
    env: HashMap<String, String>,
    client: reqwest::Client,
    state: Mutex<State>,
}

/// `num_ctx` when neither `OLLAMA_NUM_CTX` nor `/api/show` gives one.
const NUM_CTX_FALLBACK: u64 = 8192;

fn http_error(prefix: &str, res_status: reqwest::StatusCode, body: &str) -> String {
    let reason = res_status.canonical_reason().unwrap_or("");
    let body_part = if body.is_empty() {
        String::new()
    } else {
        format!(" — {body}")
    };
    format!("{prefix}: HTTP {} {reason}{body_part}", res_status.as_u16())
}

impl OllamaEmbedder {
    pub fn new(
        base_url: Option<&str>,
        model: Option<&str>,
        expected_dim: Option<usize>,
        num_ctx: Option<u64>,
        preset_name: Option<&str>,
    ) -> Self {
        Self {
            base_url: base_url.unwrap_or("http://localhost:11434").to_string(),
            model: model.unwrap_or(DEFAULT_OLLAMA_MODEL).to_string(),
            explicit_num_ctx: num_ctx,
            preset_name: preset_name.map(str::to_string),
            env: js::process_env(),
            client: reqwest::Client::new(),
            state: Mutex::new(State {
                cached_dim: expected_dim,
                ..State::default()
            }),
        }
    }

    /// Read the auto-pull switches from `env` instead of the process.
    pub fn with_env(mut self, env: HashMap<String, String>) -> Self {
        self.env = env;
        self
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn preset_name(&self) -> Option<&str> {
        self.preset_name.as_deref()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_phase(&self, phase: OllamaPhase) {
        self.state().phase = Some(phase);
    }

    /// The current lifecycle phase.
    pub fn phase(&self) -> OllamaPhase {
        self.state()
            .phase
            .clone()
            .unwrap_or(OllamaPhase::NotStarted)
    }

    /// `num_ctx` for `/api/embeddings`: the user's `OLLAMA_NUM_CTX`, else
    /// the model's real `context_length`, else 8192.
    fn effective_num_ctx(&self) -> u64 {
        self.explicit_num_ctx
            .or(self.state().cached_context_length)
            .unwrap_or(NUM_CTX_FALLBACK)
    }

    fn env_is(&self, key: &str, value: &str) -> bool {
        self.env.get(key).map(String::as_str) == Some(value)
    }

    /// The auto-pull gate:
    ///   1. `OBSIDIAN_BRAIN_OLLAMA_AUTO_PULL=0` never pulls.
    ///   2. A preset's model always pulls.
    ///   3. A model in Ollama's official `library/` namespace pulls.
    ///   4. `OBSIDIAN_BRAIN_OLLAMA_BYOM_AUTO_PULL=1` pulls anything.
    ///   5. Else refuse; init fails with the BYOM message.
    fn should_auto_pull(&self) -> bool {
        if self.env_is("OBSIDIAN_BRAIN_OLLAMA_AUTO_PULL", "0") {
            return false;
        }
        self.preset_name.is_some()
            || is_ollama_official_namespace(&self.model)
            || self.env_is("OBSIDIAN_BRAIN_OLLAMA_BYOM_AUTO_PULL", "1")
    }

    /// True when the gate refused only because the model is a third-party
    /// one without the opt-in.
    fn is_blocked_by_byom_gate(&self) -> bool {
        !self.env_is("OBSIDIAN_BRAIN_OLLAMA_AUTO_PULL", "0")
            && self.preset_name.is_none()
            && !is_ollama_official_namespace(&self.model)
            && !self.env_is("OBSIDIAN_BRAIN_OLLAMA_BYOM_AUTO_PULL", "1")
    }

    fn format_byom_blocked_message(&self) -> String {
        let namespace = ollama_namespace_of(&self.model).unwrap_or("<none>");
        let m = &self.model;
        format!(
            "Ollama model '{m}' is not pulled and auto-pull was skipped.\n\
             Reason: third-party namespace '{namespace}' is not in the auto-pull allowlist \
             (only Ollama's official 'library' models auto-pull by default for BYOM).\n\
             To fix:\n  \
             1. Run manually: ollama pull {m}\n  \
             2. Or opt in: OBSIDIAN_BRAIN_OLLAMA_BYOM_AUTO_PULL=1\n  \
             3. Or switch to a preset: EMBEDDING_PRESET=multilingual-ollama (uses qwen3-embedding:0.6b)"
        )
    }

    async fn init_inner(&self) -> anyhow::Result<()> {
        self.fetch_model_info().await?;
        self.fetch_digest().await;
        if self.state().model_not_pulled {
            if self.should_auto_pull() {
                self.pull_model().await?;
                self.set_phase(OllamaPhase::Probing);
                self.fetch_model_info().await?;
                self.fetch_digest().await;
            } else if self.is_blocked_by_byom_gate() {
                anyhow::bail!(self.format_byom_blocked_message());
            }
            // With the master switch off, the embed probe below gives the
            // "try: ollama pull" error.
        }
        // No embedding_length from /api/show and no declared dim: probe
        // with an empty embed.
        if self.state().cached_dim.is_none() {
            self.embed("", TaskType::Document).await?;
        }
        Ok(())
    }

    /// Run `/api/pull` and follow its NDJSON progress until it completes.
    async fn pull_model(&self) -> anyhow::Result<()> {
        let m = &self.model;
        tracing::info!(
            model = %m,
            auto_pull = true,
            "{m} not pulled — auto-pulling via Ollama (set OBSIDIAN_BRAIN_OLLAMA_AUTO_PULL=0 to disable)..."
        );
        let mut res = self
            .client
            .post(format!("{}/api/pull", self.base_url))
            .json(&json!({ "model": m, "stream": true }))
            .send()
            .await?;
        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}. Run `ollama pull {m}` manually, or set OBSIDIAN_BRAIN_OLLAMA_AUTO_PULL=0 to disable auto-pull.",
                http_error("Ollama auto-pull failed", status, &body)
            );
        }
        self.set_phase(OllamaPhase::Pulling {
            completed_mb: 0,
            total_mb: 0,
            pct: 0,
            phase: "starting".into(),
        });
        let mut buffer: Vec<u8> = Vec::new();
        let mut last_log: Option<Instant> = None;
        while let Some(chunk) = res.chunk().await? {
            buffer.extend_from_slice(&chunk);
            while let Some(pos) = buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buffer.drain(..=pos).collect();
                self.handle_pull_line(&String::from_utf8_lossy(&line), &mut last_log)?;
            }
        }
        tracing::info!(model = %m, "✓ pulled {m} successfully");
        Ok(())
    }

    fn handle_pull_line(&self, line: &str, last_log: &mut Option<Instant>) -> anyhow::Result<()> {
        let m = &self.model;
        if js::trim(line).is_empty() {
            return Ok(());
        }
        let Ok(parsed) = serde_json::from_str::<Value>(line) else {
            return Ok(());
        };
        if let Some(err) = parsed.get("error").filter(|e| !is_falsy(e)) {
            let err = err.as_str().map(str::to_string).unwrap_or(err.to_string());
            anyhow::bail!(
                "Ollama auto-pull failed: {err}. Run `ollama pull {m}` manually, or set OBSIDIAN_BRAIN_OLLAMA_AUTO_PULL=0 to disable auto-pull."
            );
        }
        let status = parsed.get("status").and_then(Value::as_str);
        let total = parsed.get("total").and_then(Value::as_f64);
        let completed = parsed.get("completed").and_then(Value::as_f64);
        if let (Some(total), Some(completed)) = (total, completed)
            && total > 0.0
        {
            let pct = (completed / total * 100.0).floor() as u64;
            let total_mb = (total / 1_000_000.0).floor() as u64;
            let completed_mb = (completed / 1_000_000.0).floor() as u64;
            let phase = status.unwrap_or("downloading").to_string();
            self.set_phase(OllamaPhase::Pulling {
                completed_mb,
                total_mb,
                pct,
                phase: phase.clone(),
            });
            if last_log.is_none_or(|t| t.elapsed().as_millis() >= 1000) {
                tracing::info!(
                    model = %m,
                    completed_mb,
                    total_mb,
                    pct,
                    phase = %phase,
                    "pulling {m} — {completed_mb} MB / {total_mb} MB ({pct}%)"
                );
                *last_log = Some(Instant::now());
            }
        } else if let Some(status) = status.filter(|s| !s.is_empty() && *s != "success") {
            // A phase marker: keep the latest progress numbers.
            let (completed_mb, total_mb, pct) = match self.phase() {
                OllamaPhase::Pulling {
                    completed_mb,
                    total_mb,
                    pct,
                    ..
                } => (completed_mb, total_mb, pct),
                _ => (0, 0, 0),
            };
            self.set_phase(OllamaPhase::Pulling {
                completed_mb,
                total_mb,
                pct,
                phase: status.to_string(),
            });
            tracing::info!(model = %m, phase = status, "pulling {m} — {status}");
        }
        Ok(())
    }

    /// Read dim, max tokens and the embedding capability from `/api/show`.
    /// Only the capability check fails; anything else leaves the fields
    /// unset for the embed probe.
    async fn fetch_model_info(&self) -> anyhow::Result<()> {
        self.state().model_not_pulled = false;
        let Ok(res) = self
            .client
            .post(format!("{}/api/show", self.base_url))
            .json(&json!({ "model": self.model }))
            .send()
            .await
        else {
            return Ok(());
        };
        if !res.status().is_success() {
            if res.status().as_u16() == 404 {
                self.state().model_not_pulled = true;
            }
            return Ok(());
        }
        let Ok(body) = res.json::<Value>().await else {
            return Ok(());
        };
        if let Some(caps) = body.get("capabilities").and_then(Value::as_array)
            && !caps.is_empty()
            && !caps.iter().any(|c| c.as_str() == Some("embedding"))
        {
            let shown = Value::Array(caps.clone()).to_string();
            anyhow::bail!(
                "Ollama model \"{}\" advertises capabilities {shown} but not \"embedding\". Pick an embedding-capable model (e.g. nomic-embed-text, bge-m3, mxbai-embed-large) and set EMBEDDING_MODEL accordingly.",
                self.model
            );
        }
        // The architecture prefix of the keys varies (nomic-bert, bert,
        // llama), so match on the suffix.
        if let Some(info) = body.get("model_info").and_then(Value::as_object) {
            let mut st = self.state();
            for (key, value) in info {
                let Some(n) = value.as_f64().filter(|n| n.is_finite() && *n > 0.0) else {
                    continue;
                };
                if st.cached_dim.is_none() && key.ends_with(".embedding_length") {
                    st.cached_dim = Some(n as usize);
                } else if st.cached_context_length.is_none() && key.ends_with(".context_length") {
                    st.cached_context_length = Some(n as u64);
                }
            }
        }
        Ok(())
    }

    /// Read the manifest digest of the model from `/api/tags`. Best effort.
    async fn fetch_digest(&self) {
        let Ok(res) = self
            .client
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await
        else {
            return;
        };
        if !res.status().is_success() {
            return;
        }
        let Ok(body) = res.json::<Value>().await else {
            return;
        };
        let Some(models) = body.get("models").and_then(Value::as_array) else {
            return;
        };
        // `bge-m3` matches `bge-m3:latest`, Ollama's implicit tag.
        let want_bare = if self.model.contains(':') {
            self.model.clone()
        } else {
            format!("{}:latest", self.model)
        };
        for m in models {
            let Some(name) = m
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
            else {
                continue;
            };
            if name == self.model || name == want_bare {
                if let Some(d) = m
                    .get("digest")
                    .and_then(Value::as_str)
                    .filter(|d| !d.is_empty())
                {
                    self.state().cached_digest = Some(d.to_string());
                }
                return;
            }
        }
    }

    async fn embed_inner(&self, text: &str, task: TaskType) -> anyhow::Result<Vec<f32>> {
        // Ollama applies no prefix itself; the prompt is what the model
        // sees. Authoritative metadata (override, seed) wins, even when
        // empty. Without it, or with a fallback row, the family heuristic
        // gives better defaults than no prefix.
        let meta = self.state().metadata.clone();
        let prefix = match meta
            .filter(|m| !matches!(m.prefix_source, PrefixSource::Fallback | PrefixSource::None))
        {
            Some(m) => match task {
                TaskType::Query => m.query_prefix,
                TaskType::Document => m.document_prefix,
            },
            None => self.get_prefix(task).to_string(),
        };
        let prompt = if prefix.contains("{text}") {
            prefix.replace("{text}", text)
        } else {
            format!("{prefix}{text}")
        };
        let m = &self.model;
        let res = self
            .client
            .post(format!("{}/api/embeddings", self.base_url))
            .json(&json!({
                "model": m,
                "prompt": prompt,
                "options": { "num_ctx": self.effective_num_ctx() },
            }))
            .send()
            .await?;
        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}. Is Ollama running at {} with model \"{m}\" pulled? Try: ollama pull {m}",
                http_error("Ollama embed failed", status, &body),
                self.base_url
            );
        }
        let body: Value = res.json().await?;
        let embedding: Vec<f32> = body
            .get("embedding")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|v| v.as_f64().unwrap_or(f64::NAN) as f32)
                    .collect()
            })
            .unwrap_or_default();
        if embedding.is_empty() {
            anyhow::bail!(
                "Ollama /api/embeddings returned an empty vector for model \"{m}\". The model may not be an embedding model — try \"nomic-embed-text\" or \"mxbai-embed-large\"."
            );
        }
        let mut st = self.state();
        match st.cached_dim {
            None => st.cached_dim = Some(embedding.len()),
            Some(d) if d != embedding.len() => anyhow::bail!(
                "Ollama dim mismatch: expected {d} but model \"{m}\" returned {}. Check OLLAMA_EMBEDDING_DIM matches the model.",
                embedding.len()
            ),
            Some(_) => {}
        }
        Ok(embedding)
    }

    /// Family heuristics, for when no authoritative metadata is set.
    fn get_prefix(&self, task: TaskType) -> &'static str {
        let m = self.model.to_lowercase();
        let query = task == TaskType::Query;
        if m.contains("nomic") {
            return if query {
                "search_query: "
            } else {
                "search_document: "
            };
        }
        if m.contains("e5-") {
            return if query { "query: " } else { "passage: " };
        }
        if m.contains("qwen") {
            return if query { "Query: " } else { "" };
        }
        if m.contains("mxbai") || m.contains("mixedbread") {
            return if query {
                "Represent this sentence for searching relevant passages: "
            } else {
                ""
            };
        }
        // bge-m3 and everything else: no prefix. bge-m3's dense head is
        // trained without one.
        ""
    }
}

fn is_falsy(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::String(s) => s.is_empty(),
        Value::Number(n) => n.as_f64() == Some(0.0),
        _ => false,
    }
}

#[async_trait]
impl Embedder for OllamaEmbedder {
    /// Two best-effort calls fill dim, context length and digest without
    /// a test embed: `/api/show` and `/api/tags`. A model that is not
    /// pulled is pulled first, when the gate allows.
    async fn init(&self) -> anyhow::Result<()> {
        self.state().last_error = None;
        self.set_phase(OllamaPhase::Probing);
        match self.init_inner().await {
            Ok(()) => {
                self.set_phase(OllamaPhase::Ready);
                Ok(())
            }
            Err(err) => {
                let msg = err.to_string();
                let mut st = self.state();
                st.last_error = Some(msg.clone());
                st.phase = Some(OllamaPhase::Failed { error: msg });
                Err(err)
            }
        }
    }

    async fn embed(&self, text: &str, task: TaskType) -> anyhow::Result<Vec<f32>> {
        self.state().last_error = None;
        match self.embed_inner(text, task).await {
            Ok(v) => Ok(v),
            Err(err) => {
                self.state().last_error = Some(err.to_string());
                Err(err)
            }
        }
    }

    fn dimensions(&self) -> anyhow::Result<usize> {
        let st = self.state();
        match (st.cached_dim, &st.last_error) {
            (Some(d), _) => Ok(d),
            (None, Some(err)) => Err(anyhow!(err.clone())),
            (None, None) => Err(anyhow!(
                "OllamaEmbedder dimensions not known yet — call init() or embed() once first."
            )),
        }
    }

    fn model_identifier(&self) -> String {
        format!("ollama:{}", self.model)
    }

    fn provider_name(&self) -> &'static str {
        "ollama"
    }

    /// After this, `embed` prefers these prefixes over the family
    /// heuristics.
    fn set_metadata(&self, meta: EmbedderMetadata) -> anyhow::Result<()> {
        self.state().metadata = Some(meta);
        Ok(())
    }

    fn get_metadata(&self) -> Option<EmbedderMetadata> {
        self.state().metadata.clone()
    }

    fn context_length(&self) -> Option<u64> {
        self.state().cached_context_length
    }

    fn identity_hash(&self) -> Option<String> {
        self.state().cached_digest.clone()
    }

    fn base_url(&self) -> Option<String> {
        Some(self.base_url.clone())
    }

    fn ollama_phase(&self) -> Option<OllamaPhase> {
        Some(self.phase())
    }

    /// Ollama owns the model; nothing to release here.
    async fn dispose(&self) {}
}

/// True when `model_id` is in Ollama's official `library/` namespace, where
/// auto-pull is safe by default: a bare name, or exactly one `library/`
/// segment. Anything else is a third-party namespace.
pub fn is_ollama_official_namespace(model_id: &str) -> bool {
    if !model_id.contains('/') {
        return true;
    }
    model_id
        .strip_prefix("library/")
        .is_some_and(|rest| !rest.contains('/'))
}

/// The namespace of an Ollama model id, for messages. `None` for a bare id.
pub fn ollama_namespace_of(model_id: &str) -> Option<&str> {
    model_id.rfind('/').map(|i| &model_id[..i])
}
