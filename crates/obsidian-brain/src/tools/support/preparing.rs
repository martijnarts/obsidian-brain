//! The response of a tool that needs the embedder while it is not ready.

use serde::Serialize;

use crate::context::VaultContext;
use crate::embeddings::ollama::OllamaPhase;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PreparingStatus {
    Preparing,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PreparingResponse {
    pub status: PreparingStatus,
    pub message: String,
    /// The Ollama lifecycle phase, so a client can show progress without
    /// parsing `message`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<OllamaPhase>,
}

/// `None` when the embedder is ready and the tool can go on. Otherwise a
/// `failed` response when startup failed, or a `preparing` one, with the
/// Ollama pull progress when the embedder is pulling a model. Tools that
/// need the embedder (search, reindex) send it straight back to the client.
pub fn describe_embedder_preparing(ctx: &VaultContext) -> Option<PreparingResponse> {
    if ctx.embedder_ready() {
        return None;
    }

    if let Some(err) = ctx.init_error() {
        return Some(PreparingResponse {
            status: PreparingStatus::Failed,
            message: format!(
                "Embedding model failed to load: {err}. Restart the MCP server to retry. For diagnosis, run 'obsidian-brain models check <model-id>' on the command line."
            ),
            phase: None,
        });
    }

    // Only the Ollama embedder has a phase; the generic message covers the rest.
    let phase = ctx.embedder.ollama_phase();
    match &phase {
        Some(OllamaPhase::Pulling {
            completed_mb,
            total_mb,
            pct,
            phase: step,
        }) => {
            return Some(PreparingResponse {
                status: PreparingStatus::Preparing,
                message: format!(
                    "Embedding model pull in progress: {completed_mb} MB / {total_mb} MB ({pct}%) — {step}. Retry once the pull completes."
                ),
                phase,
            });
        }
        Some(OllamaPhase::Failed { error }) => {
            return Some(PreparingResponse {
                status: PreparingStatus::Failed,
                message: format!(
                    "Embedding model failed to load: {error}. Restart the MCP server to retry."
                ),
                phase,
            });
        }
        _ => {}
    }

    let message = if ctx.reindex_in_progress() {
        "Re-embedding your vault against the current embedder (this happens when the embedding model changes or its prefix strategy is updated). Search/reindex will resume automatically."
    } else {
        "Embedding model is still initialising on first run (~34MB local download, or a multi-hundred-MB Ollama pull). Retry shortly."
    };
    Some(PreparingResponse {
        status: PreparingStatus::Preparing,
        message: message.to_string(),
        phase,
    })
}
