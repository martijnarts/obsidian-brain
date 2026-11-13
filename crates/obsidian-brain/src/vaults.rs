//! The vaults one server serves, by name, and their lifecycle: open, the
//! startup index, the watchers, and close.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::config::VaultConfig;
use crate::context::VaultContext;
use crate::embeddings::{SharedEmbedder, create_embedder, share_embedder};
use crate::pipeline::{WatcherHandle, WatcherOptions, start_watcher};
use crate::store::nodes::all_node_ids;

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

struct Vault {
    name: String,
    ctx: Arc<VaultContext>,
    watcher: Mutex<Option<WatcherHandle>>,
}

pub struct Vaults {
    vaults: Vec<Vault>,
    shared: SharedEmbedder,
}

impl Vaults {
    /// Open a context per vault, each keeping its index in
    /// `<data_dir>/<name>`. All vaults share one embedder, built from the
    /// environment, so the model loads once.
    pub async fn open(specs: &[VaultSpec], data_dir: &Path) -> anyhow::Result<Self> {
        let shared = share_embedder(create_embedder(data_dir)?);
        Self::open_with(specs, data_dir, shared).await
    }

    /// [`Self::open`] with a given embedder. If one vault fails to open,
    /// the ones already open are closed before the error returns.
    pub async fn open_with(
        specs: &[VaultSpec],
        data_dir: &Path,
        shared: SharedEmbedder,
    ) -> anyhow::Result<Self> {
        let mut seen = std::collections::HashSet::new();
        for spec in specs {
            if !seen.insert(&spec.name) {
                anyhow::bail!("vault name {:?} is given twice", spec.name);
            }
        }
        let mut out = Self {
            vaults: Vec::new(),
            shared: shared.clone(),
        };
        for spec in specs {
            let config = VaultConfig::new(&spec.name, &spec.path, data_dir);
            match VaultContext::open(config, Some(shared.clone())).await {
                Ok(ctx) => out.vaults.push(Vault {
                    name: spec.name.clone(),
                    ctx,
                    watcher: Mutex::new(None),
                }),
                Err(err) => {
                    out.close().await;
                    return Err(err.context(format!("could not open vault {:?}", spec.name)));
                }
            }
        }
        Ok(out)
    }

    pub fn names(&self) -> Vec<String> {
        self.vaults.iter().map(|v| v.name.clone()).collect()
    }

    pub fn get(&self, name: &str) -> Option<Arc<VaultContext>> {
        self.vaults
            .iter()
            .find(|v| v.name == name)
            .map(|v| v.ctx.clone())
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<VaultContext>> {
        self.vaults.iter().map(|v| &v.ctx)
    }

    /// The one embedder every vault uses.
    pub fn shared_embedder(&self) -> &SharedEmbedder {
        &self.shared
    }

    /// The `list_vaults` result.
    pub fn describe(&self) -> Value {
        Value::Array(
            self.vaults
                .iter()
                .map(|v| {
                    let ctx = &v.ctx;
                    let mut out = json!({
                        "name": v.name,
                        "path": ctx.config.vault_path,
                        "notes": ctx.note_count(),
                        "embedderReady": ctx.embedder_ready(),
                        "reindexInProgress": ctx.reindex_in_progress(),
                    });
                    if let Some(err) = ctx.init_error() {
                        out["initError"] = err.into();
                    }
                    out
                })
                .collect(),
        )
    }

    /// Start a watcher per vault, unless `OBSIDIAN_BRAIN_NO_WATCH=1`. A
    /// watcher that cannot start is logged; the others still start.
    pub fn start_watchers(&self, opts: WatcherOptions) {
        if std::env::var("OBSIDIAN_BRAIN_NO_WATCH").as_deref() == Ok("1") {
            return;
        }
        self.watch_all(opts);
    }

    /// Start a watcher per vault, whatever the environment says.
    pub fn watch_all(&self, opts: WatcherOptions) {
        for v in &self.vaults {
            match start_watcher(v.ctx.clone(), opts) {
                Ok(handle) => {
                    *v.watcher.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
                }
                Err(err) => {
                    tracing::error!(vault = %v.name, error = %err, "watcher error: {err}");
                }
            }
        }
    }

    /// True when the vault has a running watcher.
    pub fn is_watching(&self, name: &str) -> bool {
        self.vaults.iter().any(|v| {
            v.name == name
                && v.watcher
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_some()
        })
    }

    /// Close every vault's watcher, then its context, then dispose the
    /// shared embedder once.
    pub async fn close(&self) {
        for v in &self.vaults {
            let watcher = v.watcher.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(w) = watcher {
                w.close().await;
            }
            v.ctx.close().await;
        }
        self.shared.embedder.dispose().await;
    }
}

/// Run the startup index of each vault in turn, so the memory peaks of two
/// full indexes never add up. A tool call on a vault that waits its turn
/// still works: it loads the embedder on demand. Stops early once
/// `stopped()` returns true.
pub async fn index_vaults_in_turn<'a, I, F, Fut>(
    vaults: I,
    run_startup_index: F,
    stopped: impl Fn() -> bool,
) where
    I: IntoIterator<Item = &'a Arc<VaultContext>>,
    F: Fn(Arc<VaultContext>, bool) -> Fut,
    Fut: Future<Output = ()>,
{
    for ctx in vaults {
        if stopped() {
            return;
        }
        let db_is_empty = ctx
            .db
            .with(|c| all_node_ids(c).map(|ids| ids.is_empty()))
            .unwrap_or(true);
        run_startup_index(ctx.clone(), db_is_empty).await;
        ctx.wait_for_background_reindex().await;
    }
}

/// The background startup work of one vault: load the embedder, run the
/// compatibility check, and queue the first index (empty index) or an
/// incremental catch-up (`OBSIDIAN_BRAIN_NO_CATCHUP=1` skips it). Returns
/// once the index work is queued. A failure is recorded in
/// `ctx.init_error()` instead of returned.
pub async fn run_startup_index(ctx: Arc<VaultContext>, db_is_empty: bool) {
    if let Err(err) = startup_index(&ctx, db_is_empty).await {
        ctx.set_init_error(&err);
        tracing::error!(error = %err, "background init failed: {err:#}");
    }
}

async fn startup_index(ctx: &Arc<VaultContext>, db_is_empty: bool) -> anyhow::Result<()> {
    if db_is_empty {
        tracing::info!(
            first_time = true,
            "index is empty, running first-time index. Time depends on vault size — typically under a minute for small vaults, a few minutes for thousands of notes. Downloads embedding model on first boot."
        );
        ctx.ensure_embedder_ready().await?;
        let job = ctx.clone();
        ctx.enqueue_background_reindex(async move {
            let stats = job.pipeline.index(&job.config.vault_path).await?;
            tracing::info!(
                nodes_indexed = stats.nodes_indexed,
                edges_indexed = stats.edges_indexed,
                "indexed {} notes, {} links.",
                stats.nodes_indexed,
                stats.edges_indexed
            );
            Ok(())
        });
        return Ok(());
    }

    // A non-empty index: say why a reindex starts (a model change, the
    // chunk upgrade, the FTS tokenizer swap). A full reindex clears the
    // sync mtimes, which the mtime guard would otherwise trust.
    ctx.ensure_embedder_ready().await?;
    let boot = ctx.bootstrap_result();
    if let Some(boot) = &boot {
        for reason in &boot.reasons {
            tracing::info!(bootstrap_reason = true, "{reason}");
        }
        if boot.needs_reindex {
            ctx.db.with(|c| c.execute_batch("DELETE FROM sync"))?;
            tracing::info!(
                rebuild_all = true,
                "rebuilding per-chunk embeddings (may take a minute)..."
            );
        }
    }

    // Notes may have changed while the server was down. Catch up in the
    // background; the watcher takes over live edits from here.
    let from_scratch = boot.is_some_and(|b| b.needs_reindex);
    if std::env::var("OBSIDIAN_BRAIN_NO_CATCHUP").as_deref() != Ok("1") {
        let job = ctx.clone();
        ctx.enqueue_background_reindex(async move {
            let stats = job.pipeline.index(&job.config.vault_path).await?;
            if stats.nodes_indexed > 0 {
                let (suffix, reason) = if from_scratch {
                    (
                        "re-embedded after model/schema change",
                        "model-or-schema-change",
                    )
                } else {
                    ("modified while the server was down", "edits-while-down")
                };
                tracing::info!(
                    nodes_indexed = stats.nodes_indexed,
                    reason,
                    "startup catchup — reindexed {} note(s) ({suffix})",
                    stats.nodes_indexed
                );
            }
            Ok(())
        });
    }
    Ok(())
}
