//! The pluggable embedder contract and the metadata it carries.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::ollama::OllamaPhase;

/// Which side of an asymmetric model a text is for. bge-* and e5-* use a
/// different prefix for documents and queries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskType {
    #[default]
    Document,
    Query,
}

/// Where a model's prefixes came from, as persisted in `prefix_source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrefixSource {
    Override,
    Seed,
    Metadata,
    MetadataBase,
    Readme,
    Fallback,
    None,
}

impl PrefixSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::Seed => "seed",
            Self::Metadata => "metadata",
            Self::MetadataBase => "metadata-base",
            Self::Readme => "readme",
            Self::Fallback => "fallback",
            Self::None => "none",
        }
    }

    /// Parse a stored value. Unknown text reads as `None`.
    pub fn parse(s: &str) -> Self {
        match s {
            "override" => Self::Override,
            "seed" => Self::Seed,
            "metadata" => Self::Metadata,
            "metadata-base" => Self::MetadataBase,
            "readme" => Self::Readme,
            "fallback" => Self::Fallback,
            _ => Self::None,
        }
    }
}

/// Resolved model metadata, pushed into an embedder so it applies the right
/// query and document prefix. Mirrors `ResolvedMetadata` in the resolver.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbedderMetadata {
    pub model_id: String,
    pub dim: Option<usize>,
    pub max_tokens: u64,
    pub query_prefix: String,
    pub document_prefix: String,
    pub prefix_source: PrefixSource,
    pub base_model: Option<String>,
    pub size_bytes: Option<i64>,
}

/// Turns text into a normalised vector. The concrete runtime (ONNX, a remote
/// HTTP service) stays behind this trait, so callers never depend on it.
#[async_trait]
pub trait Embedder: Send + Sync {
    /// Load the backing model or runtime. Call it before `embed`.
    async fn init(&self) -> anyhow::Result<()>;

    /// Embed one text. Call it after `init`.
    async fn embed(&self, text: &str, task: TaskType) -> anyhow::Result<Vec<f32>>;

    /// Output dimensionality, the same for every vector. Errors before the
    /// dimension is known.
    fn dimensions(&self) -> anyhow::Result<usize>;

    /// Stable model id, persisted in the index so a model change reindexes.
    fn model_identifier(&self) -> String;

    /// Short backend name, such as `transformers.js` or `ollama`.
    fn provider_name(&self) -> &'static str;

    /// Store resolved metadata, so `embed` applies the right prefix.
    fn set_metadata(&self, _meta: EmbedderMetadata) -> anyhow::Result<()> {
        Ok(())
    }

    /// The metadata in effect, or `None`.
    fn get_metadata(&self) -> Option<EmbedderMetadata> {
        None
    }

    /// The model's architectural context window, when the backend reports
    /// it live (Ollama's `/api/show`).
    fn context_length(&self) -> Option<u64> {
        None
    }

    /// A content address for the loaded weights, when the backend has one
    /// (the Ollama manifest digest). A change triggers a reindex.
    fn identity_hash(&self) -> Option<String> {
        None
    }

    /// The tokenizer's `model_max_length`, recorded when the model loaded.
    /// `f64::INFINITY` when the tokenizer sets none. Only the ONNX embedder
    /// has one.
    fn model_max_length(&self) -> Option<f64> {
        None
    }

    /// The Ollama server URL, for the capacity probe.
    fn base_url(&self) -> Option<String> {
        None
    }

    /// The Ollama lifecycle phase, so tools can show pull progress.
    fn ollama_phase(&self) -> Option<OllamaPhase> {
        None
    }

    /// Release the model and its memory.
    async fn dispose(&self);
}
