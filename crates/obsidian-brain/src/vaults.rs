//! The vaults one server serves, by name.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::config::VaultConfig;
use crate::context::VaultContext;

/// One `--vault name=path` flag.
#[derive(Debug, Clone, PartialEq)]
pub struct VaultSpec {
    pub name: String,
    pub path: PathBuf,
}

impl std::str::FromStr for VaultSpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (name, path) = s
            .split_once('=')
            .ok_or_else(|| format!("expected name=path, got {s:?}"))?;
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!(
                "vault name {name:?} must be letters, digits, '-' or '_'"
            ));
        }
        if path.is_empty() {
            return Err(format!("vault {name:?} has no path"));
        }
        Ok(Self {
            name: name.to_string(),
            path: PathBuf::from(path),
        })
    }
}

pub struct Vaults {
    vaults: Vec<(String, Arc<VaultContext>)>,
}

impl Vaults {
    /// Opens a context per vault. If one fails, the ones already open are
    /// closed before the error returns.
    pub async fn open(specs: &[VaultSpec], data_dir: &Path) -> anyhow::Result<Self> {
        let mut seen = std::collections::HashSet::new();
        for spec in specs {
            if !seen.insert(&spec.name) {
                anyhow::bail!("vault name {:?} is given twice", spec.name);
            }
        }
        let mut vaults = Vec::new();
        for spec in specs {
            let config = VaultConfig::new(&spec.name, &spec.path, data_dir);
            match VaultContext::open(config).await {
                Ok(ctx) => vaults.push((spec.name.clone(), Arc::new(ctx))),
                Err(err) => {
                    Self { vaults }.close().await;
                    return Err(err.context(format!("could not open vault {:?}", spec.name)));
                }
            }
        }
        Ok(Self { vaults })
    }

    pub fn names(&self) -> Vec<String> {
        self.vaults.iter().map(|(n, _)| n.clone()).collect()
    }

    pub fn get(&self, name: &str) -> Option<Arc<VaultContext>> {
        self.vaults
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, c)| c.clone())
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<VaultContext>> {
        self.vaults.iter().map(|(_, c)| c)
    }

    /// The `list_vaults` result.
    pub fn describe(&self) -> Value {
        Value::Array(
            self.vaults
                .iter()
                .map(|(name, ctx)| {
                    let mut v = json!({
                        "name": name,
                        "path": ctx.config.vault_path,
                        "notes": ctx.note_count(),
                        "embedderReady": ctx.embedder_ready(),
                        "reindexInProgress": ctx.reindex_in_progress(),
                    });
                    if let Some(err) = ctx.init_error() {
                        v["initError"] = err.into();
                    }
                    v
                })
                .collect(),
        )
    }

    pub async fn close(&self) {
        for (_, ctx) in &self.vaults {
            ctx.close().await;
        }
    }
}
