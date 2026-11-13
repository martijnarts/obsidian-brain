//! The runtime state of one vault that every tool handler needs, built
//! once at startup.
//!
//! The embedder is created but not loaded: [`VaultContext::ensure_embedder_ready`]
//! loads it on first use, because the first load may download a model.
//! Several vaults share one embedder through [`SharedEmbedder`], so the
//! model loads once.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use anyhow::anyhow;
use tokio::sync::{mpsc, watch};

use crate::config::VaultConfig;
use crate::embeddings::metadata_resolver::{ResolverDeps, resolve_model_metadata};
use crate::embeddings::{Embedder, SharedEmbedder, create_embedder, share_embedder};
use crate::pipeline::{BootstrapResult, IndexPipeline, bootstrap};
use crate::search::Search;
use crate::store::Db;
use crate::store::db::ensure_vec_tables;
use crate::store::nodes::all_node_ids;
use crate::vault::writer::VaultWriter;

type Job = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;

/// How long [`VaultContext::close`] waits for queued indexing.
const CLOSE_DRAIN: Duration = Duration::from_secs(3);

/// The background reindex queue: one worker runs the jobs in FIFO order.
struct ReindexQueue {
    tx: mpsc::UnboundedSender<Job>,
    /// Jobs queued or running.
    pending: watch::Sender<usize>,
    in_progress: Arc<AtomicBool>,
    enqueued: AtomicUsize,
}

impl ReindexQueue {
    fn start() -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Job>();
        let (pending, _) = watch::channel(0usize);
        let in_progress = Arc::new(AtomicBool::new(false));
        let worker_pending = pending.clone();
        let worker_flag = in_progress.clone();
        tokio::spawn(async move {
            while let Some(job) = rx.recv().await {
                worker_flag.store(true, Ordering::SeqCst);
                // Its own task, so a panic in one job does not stop the queue.
                match tokio::spawn(job).await {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        tracing::error!(error = %err, "background reindex failed: {err}");
                    }
                    Err(err) => {
                        tracing::error!(error = %err, "background reindex failed: {err}");
                    }
                }
                worker_flag.store(false, Ordering::SeqCst);
                worker_pending.send_modify(|n| *n = n.saturating_sub(1));
            }
        });
        Self {
            tx,
            pending,
            in_progress,
            enqueued: AtomicUsize::new(0),
        }
    }
}

/// The state of the embedder load: `None` until it finishes.
type InitState = Option<Result<(), String>>;

/// One open vault.
pub struct VaultContext {
    pub config: VaultConfig,
    pub db: Db,
    /// Created but not loaded; call [`Self::ensure_embedder_ready`] first.
    pub embedder: Arc<dyn Embedder>,
    pub search: Search,
    pub writer: VaultWriter,
    pub pipeline: IndexPipeline,
    shared: SharedEmbedder,
    owns_embedder: bool,
    this: Weak<VaultContext>,
    init_started: AtomicBool,
    init: watch::Sender<InitState>,
    embedder_ready: AtomicBool,
    bootstrap: Mutex<Option<BootstrapResult>>,
    init_error: Mutex<Option<String>>,
    reindex: ReindexQueue,
    last_manual_reindex_reason: Mutex<Option<String>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl VaultContext {
    /// Open the vault's index under `config.data_dir`. With `shared`, the
    /// context uses that embedder and leaves its disposal to whoever shared
    /// it; without, it builds and owns one. Must run inside a Tokio runtime:
    /// the background reindex worker starts here.
    pub async fn open(
        config: VaultConfig,
        shared: Option<SharedEmbedder>,
    ) -> anyhow::Result<Arc<Self>> {
        std::fs::create_dir_all(&config.data_dir)?;
        let db = Db::open(&config.db_path)?;
        let (shared, owns) = match shared {
            Some(s) => (s, false),
            None => {
                let top = config.data_dir.parent().unwrap_or(&config.data_dir);
                (share_embedder(create_embedder(top)?), true)
            }
        };
        Ok(Self::from_parts(config, db, shared, owns))
    }

    /// A context over an index that is already open, for tests and for
    /// callers that build their own store. Must run inside a Tokio runtime.
    pub fn from_parts(
        config: VaultConfig,
        db: Db,
        shared: SharedEmbedder,
        owns_embedder: bool,
    ) -> Arc<Self> {
        let embedder = shared.embedder.clone();
        Arc::new_cyclic(|this| Self {
            search: Search::new(db.clone(), embedder.clone()),
            writer: VaultWriter::new(config.vault_path.clone()),
            pipeline: IndexPipeline::new(db.clone(), embedder.clone()),
            config,
            db,
            embedder,
            shared,
            owns_embedder,
            this: this.clone(),
            init_started: AtomicBool::new(false),
            init: watch::channel(None).0,
            embedder_ready: AtomicBool::new(false),
            bootstrap: Mutex::new(None),
            init_error: Mutex::new(None),
            reindex: ReindexQueue::start(),
            last_manual_reindex_reason: Mutex::new(None),
        })
    }

    /// False when the embedder is shared with other vaults; closing this
    /// context then leaves it alone.
    pub fn owns_embedder(&self) -> bool {
        self.owns_embedder
    }

    /// Load the embedder, resolve its metadata (cache, seed, fallbacks),
    /// reconcile the index with it ([`bootstrap`]) and size the vec tables.
    ///
    /// The first call starts the load in a task of its own; every caller,
    /// concurrent or later, waits for that one load and gets its outcome.
    /// A caller that gives up does not cancel it. A failure is recorded in
    /// [`Self::init_error`] whether or not anyone waits.
    pub async fn ensure_embedder_ready(&self) -> anyhow::Result<()> {
        if !self.init_started.swap(true, Ordering::SeqCst)
            && let Some(ctx) = self.this.upgrade()
        {
            tokio::spawn(async move {
                let outcome = ctx.load_embedder().await.map_err(|e| format!("{e:#}"));
                if let Err(msg) = &outcome {
                    // The first error wins.
                    lock(&ctx.init_error).get_or_insert_with(|| format!("Error: {msg}"));
                }
                ctx.init.send_replace(Some(outcome));
            });
        }
        let mut rx = self.init.subscribe();
        let state = rx
            .wait_for(Option::is_some)
            .await
            .map_err(|_| anyhow!("embedder load was dropped"))?;
        match state.as_ref() {
            Some(Ok(())) => Ok(()),
            Some(Err(msg)) => Err(anyhow!("{msg}")),
            None => unreachable!("wait_for returns a loaded state"),
        }
    }

    async fn load_embedder(&self) -> anyhow::Result<()> {
        self.shared.init().await.map_err(|e| anyhow!("{e:#}"))?;
        let dim = self.embedder.dimensions()?;
        tracing::debug!(dim, "embedder loaded");

        let model = self.embedder.model_identifier();
        let embedder = self.embedder.clone();
        let meta = self
            .db
            .run(move |c| {
                let deps = ResolverDeps {
                    embedder: Some(&*embedder),
                    ..ResolverDeps::new(c)
                };
                resolve_model_metadata(&model, &deps)
            })
            .await?;
        self.embedder.set_metadata(meta.to_embedder_metadata())?;

        // Reconcile the stored embedder identity with the live one before
        // anything writes vectors.
        let embedder = self.embedder.clone();
        let result = self
            .db
            .run(move |c| -> anyhow::Result<BootstrapResult> {
                let result = bootstrap(c, &*embedder)?;
                ensure_vec_tables(c, dim)?;
                Ok(result)
            })
            .await?;
        *lock(&self.bootstrap) = Some(result);
        self.embedder_ready.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// True once [`Self::ensure_embedder_ready`] has succeeded.
    pub fn embedder_ready(&self) -> bool {
        self.embedder_ready.load(Ordering::SeqCst)
    }

    /// The last startup compatibility check. `None` until the embedder has
    /// loaded: the check needs the embedder's dimensions.
    pub fn bootstrap_result(&self) -> Option<BootstrapResult> {
        lock(&self.bootstrap).clone()
    }

    /// Why the embedder or the startup index failed, as JS `String(err)`
    /// showed it: `Error: <message>`.
    pub fn init_error(&self) -> Option<String> {
        lock(&self.init_error).clone()
    }

    /// Record a startup failure. Unlike the embedder load, this replaces an
    /// earlier error.
    pub fn set_init_error(&self, message: impl std::fmt::Display) {
        *lock(&self.init_error) = Some(format!("Error: {message}"));
    }

    /// Queue `work` behind every reindex queued before it. The jobs run one
    /// at a time, in order, on a background task; a failed job is logged
    /// and never blocks the next. Write tools return at once and the index
    /// catches up here.
    pub fn enqueue_background_reindex<F>(&self, work: F)
    where
        F: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        self.reindex.enqueued.fetch_add(1, Ordering::SeqCst);
        self.reindex.pending.send_modify(|n| *n += 1);
        if self.reindex.tx.send(Box::pin(work)).is_err() {
            self.reindex
                .pending
                .send_modify(|n| *n = n.saturating_sub(1));
        }
    }

    /// Wait until every queued reindex has finished. Tests await this
    /// before they tear a vault down; shutdown awaits it with a deadline.
    pub async fn wait_for_background_reindex(&self) {
        let mut rx = self.reindex.pending.subscribe();
        let _ = rx.wait_for(|n| *n == 0).await;
    }

    /// How many reindex jobs were ever queued on this context.
    pub fn background_reindex_count(&self) -> usize {
        self.reindex.enqueued.load(Ordering::SeqCst)
    }

    /// True while a background reindex runs, such as the re-embedding after
    /// a model change. Search uses it to say "re-embedding" rather than
    /// "still downloading".
    pub fn reindex_in_progress(&self) -> bool {
        self.reindex.in_progress.load(Ordering::SeqCst)
    }

    /// The reason given to the last `reindex` tool call, for
    /// `index_status`. `None` until the first manual reindex; kept in memory
    /// only.
    pub fn last_manual_reindex_reason(&self) -> Option<String> {
        lock(&self.last_manual_reindex_reason).clone()
    }

    pub fn set_last_manual_reindex_reason(&self, reason: Option<String>) {
        *lock(&self.last_manual_reindex_reason) = reason;
    }

    /// The number of nodes in the index, stubs included.
    pub fn note_count(&self) -> usize {
        self.db
            .with(|c| all_node_ids(c).map(|ids| ids.len()))
            .unwrap_or(0)
    }

    /// Wait up to 3 seconds for queued indexing, dispose the embedder when
    /// this context owns it and it loaded, and checkpoint the WAL. The
    /// connection itself closes when the last handle drops.
    pub async fn close(&self) {
        let _ = tokio::time::timeout(CLOSE_DRAIN, self.wait_for_background_reindex()).await;
        if self.embedder_ready() && self.owns_embedder {
            self.embedder.dispose().await;
        }
        let checkpoint = self
            .db
            .with(|c| c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())));
        if let Err(err) = checkpoint {
            tracing::warn!(error = %err, "WAL checkpoint failed during shutdown (ignored): {err}");
        }
    }
}
