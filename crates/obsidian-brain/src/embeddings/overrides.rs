//! User-controlled model metadata overrides from
//! `~/.config/obsidian-brain/model-overrides.json`.
//!
//! Each override patches the resolved metadata of one model id: just
//! `maxTokens`, just `queryPrefix`, or any combination. Omitted fields keep
//! the underlying value.
//!
//! ```json
//! { "$version": 1,
//!   "models": { "MongoDB/mdbr-leaf-ir": { "maxTokens": 1024 } } }
//! ```
//!
//! A bad shape or a missing file yields an empty map and a warning. A bad
//! field is dropped, and the rest of its entry still loads.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::user_config::get_overrides_path;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelOverride {
    pub max_tokens: Option<u64>,
    /// `Some(None)` clears the prefix on purpose; `None` leaves it alone.
    pub query_prefix: Option<Option<String>>,
    pub document_prefix: Option<Option<String>>,
}

pub type Overrides = HashMap<String, ModelOverride>;

const SUPPORTED_VERSION: i64 = 1;

static CACHED: Mutex<Option<Arc<Overrides>>> = Mutex::new(None);

/// Reset the in-process cache.
pub fn reset_overrides_cache() {
    *CACHED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn validate_entry(model_id: &str, raw: &Value) -> Option<ModelOverride> {
    let r = raw.as_object()?;
    let mut out = ModelOverride::default();
    let mut has_field = false;

    if let Some(v) = r.get("maxTokens") {
        match v.as_f64() {
            Some(n) if n.is_finite() && n > 0.0 => {
                out.max_tokens = Some(n.floor() as u64);
                has_field = true;
            }
            _ => tracing::warn!(
                model_id,
                field = "maxTokens",
                "model-overrides: {model_id}.maxTokens must be a positive finite number — dropping"
            ),
        }
    }
    for key in ["queryPrefix", "documentPrefix"] {
        let Some(v) = r.get(key) else { continue };
        let parsed = match v {
            Value::Null => Some(None),
            Value::String(s) => Some(Some(s.clone())),
            _ => None,
        };
        match parsed {
            Some(p) => {
                if key == "queryPrefix" {
                    out.query_prefix = Some(p);
                } else {
                    out.document_prefix = Some(p);
                }
                has_field = true;
            }
            None => tracing::warn!(
                model_id,
                field = key,
                "model-overrides: {model_id}.{key} must be string|null — dropping"
            ),
        }
    }
    has_field.then_some(out)
}

/// Read and validate an overrides file, without the cache.
pub fn load_overrides_from(path: &Path) -> Overrides {
    let mut out = Overrides::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return out;
    };
    let shown = path.display().to_string();
    let parsed: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(
                path = %shown,
                error = %err,
                "model-overrides: invalid JSON at {shown} ({err}) — ignoring"
            );
            return out;
        }
    };
    let Some(file) = parsed.as_object() else {
        tracing::warn!(
            path = %shown,
            "model-overrides: top-level value at {shown} is not an object — ignoring"
        );
        return out;
    };
    let version = file.get("$version");
    if version.and_then(Value::as_i64) != Some(SUPPORTED_VERSION) {
        let v = version.map(Value::to_string).unwrap_or_else(|| "?".into());
        tracing::warn!(
            "model-overrides: unsupported $version {v} (expected {SUPPORTED_VERSION}) — ignoring"
        );
        return out;
    }
    let Some(models) = file.get("models").and_then(Value::as_object) else {
        return out;
    };
    for (id, raw) in models {
        if let Some(validated) = validate_entry(id, raw) {
            out.insert(id.clone(), validated);
        }
    }
    out
}

/// Load the overrides once per process.
pub fn load_overrides() -> Arc<Overrides> {
    let mut guard = CACHED.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .get_or_insert_with(|| Arc::new(load_overrides_from(&get_overrides_path())))
        .clone()
}
