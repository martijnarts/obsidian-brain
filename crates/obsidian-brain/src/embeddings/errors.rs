//! Actionable load errors for the ONNX embedder.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EmbedderLoadErrorKind {
    NotFound,
    NoOnnx,
    Offline,
    Unknown,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct EmbedderLoadError {
    pub kind: EmbedderLoadErrorKind,
    pub message: String,
    /// The raw error, rendered with its source chain.
    pub cause: Option<String>,
}

impl EmbedderLoadError {
    pub fn new(kind: EmbedderLoadErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            cause: None,
        }
    }
}

static OFFLINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)ENOTFOUND|ECONNREFUSED|Failed to fetch|getaddrinfo|network|ETIMEDOUT|error sending request|dns error|connection refused|timed out",
    )
    .unwrap()
});
static ONNX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\.onnx").unwrap());
static MISSING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(Could not locate|no such file|not found|404)").unwrap());
static NOT_FOUND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(404|401|not found|does not exist|repository.*not found)").unwrap()
});

/// Map a raw download or ONNX Runtime error onto an actionable
/// `EmbedderLoadError`. An `EmbedderLoadError` passes through unchanged.
pub fn classify_load_error(model_id: &str, err: anyhow::Error) -> EmbedderLoadError {
    let err = match err.downcast::<EmbedderLoadError>() {
        Ok(e) => return e,
        Err(e) => e,
    };
    let msg = format!("{err:#}");
    let with_cause = |kind, message: String| EmbedderLoadError {
        kind,
        message,
        cause: Some(msg.clone()),
    };

    // Offline first, so a network error body that mentions 404 still reads
    // as offline.
    if OFFLINE.is_match(&msg) {
        return with_cause(
            EmbedderLoadErrorKind::Offline,
            format!(
                "Cannot reach Hugging Face to download '{model_id}'. Connect to the internet and retry."
            ),
        );
    }

    // Missing ONNX weights, before the generic 404: the URL names the file.
    if ONNX.is_match(&msg) && MISSING.is_match(&msg) {
        let base = model_id.rsplit('/').next().unwrap_or(model_id);
        return with_cause(
            EmbedderLoadErrorKind::NoOnnx,
            format!(
                "Model '{model_id}' has no ONNX weights. transformers.js requires ONNX — use the Xenova port (try 'Xenova/{base}'), or set EMBEDDING_PROVIDER=ollama to switch providers."
            ),
        );
    }

    if NOT_FOUND.is_match(&msg) {
        return with_cause(
            EmbedderLoadErrorKind::NotFound,
            format!(
                "Model '{model_id}' not found on Hugging Face. Check spelling — the Xenova/* namespace is the most reliable source of ONNX-converted models."
            ),
        );
    }

    tracing::error!(
        model_id,
        error = %msg,
        "unmapped embedder error (please report this message verbatim to github.com/sweir1/obsidian-brain/issues): {msg}"
    );
    with_cause(
        EmbedderLoadErrorKind::Unknown,
        format!("Failed to load embedding model '{model_id}': {msg}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn classifies_not_found() {
        let e = classify_load_error("MyOrg/my-model", anyhow!("404 not found: MyOrg/my-model"));
        assert_eq!(e.kind, EmbedderLoadErrorKind::NotFound);
        assert!(e.message.contains("MyOrg/my-model"));
    }

    #[test]
    fn classifies_hf_hub_missing_repo_as_not_found() {
        let e = classify_load_error(
            "MyOrg/my-model",
            anyhow!(
                "request error: HTTP status client error (401 Unauthorized) for url (https://huggingface.co/MyOrg/my-model/resolve/main/config.json)"
            ),
        );
        assert_eq!(e.kind, EmbedderLoadErrorKind::NotFound);
    }

    #[test]
    fn classifies_no_onnx() {
        let e = classify_load_error(
            "SomeOrg/model",
            anyhow!("Could not locate file: model.onnx for model SomeOrg/model"),
        );
        assert_eq!(e.kind, EmbedderLoadErrorKind::NoOnnx);
        assert!(e.message.contains("SomeOrg/model"));
        assert!(e.message.contains("ONNX"));
        assert!(e.message.contains("Xenova/model"));
    }

    #[test]
    fn classifies_offline() {
        let e = classify_load_error(
            "Xenova/bge-small-en-v1.5",
            anyhow!("getaddrinfo ENOTFOUND huggingface.co"),
        );
        assert_eq!(e.kind, EmbedderLoadErrorKind::Offline);
        assert!(e.message.contains("Xenova/bge-small-en-v1.5"));
        let e = classify_load_error(
            "x/y",
            anyhow!("request error: error sending request for url (https://huggingface.co/x)"),
        );
        assert_eq!(e.kind, EmbedderLoadErrorKind::Offline);
    }

    #[test]
    fn classifies_unknown_with_original_message() {
        let e = classify_load_error("x/y", anyhow!("something weird happened"));
        assert_eq!(e.kind, EmbedderLoadErrorKind::Unknown);
        assert!(e.message.contains("something weird happened"));
        assert_eq!(e.cause.as_deref(), Some("something weird happened"));
    }

    #[test]
    fn passes_an_existing_load_error_through() {
        let original = EmbedderLoadError::new(EmbedderLoadErrorKind::Offline, "custom");
        let e = classify_load_error("x/y", anyhow::Error::new(original));
        assert_eq!(e.kind, EmbedderLoadErrorKind::Offline);
        assert_eq!(e.message, "custom");
    }
}
