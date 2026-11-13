//! Watch a vault and keep its index live.
//!
//! A change is indexed once the file has been quiet for a while: writes
//! must settle for `stability_ms` (2 s, as chokidar's `awaitWriteFinish`),
//! then a per-file debounce of `debounce_ms` (3 s) collapses bursts such as
//! Obsidian's autosave into one reindex per editing pause. A delete waits
//! for the debounce only. Events that existed before the watcher started
//! are not replayed.
//!
//! Ignored directories (`.obsidian`, `.trash`, `.git`, `node_modules`,
//! `attachments`) are never watched, and only `.md` files are indexed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use notify::event::{CreateKind, ModifyKind, RenameMode};
use notify::{EventKind, RecursiveMode, Watcher};
use regex::Regex;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::indexer::NoteEvent;
use crate::context::VaultContext;
use crate::util::js;

const DEFAULT_DEBOUNCE_MS: u64 = 3_000;
const DEFAULT_STABILITY_MS: u64 = 2_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WatcherOptions {
    /// Per-file reindex debounce in ms. Collapses bursts of writes from
    /// Obsidian's autosave into one reindex.
    pub debounce_ms: Option<u64>,
    /// How long a file's writes must have settled before its debounce
    /// starts, in ms. Not configurable from the environment; tests shorten
    /// it.
    pub stability_ms: Option<u64>,
}

/// `OBSIDIAN_BRAIN_WATCH_DEBOUNCE_MS`, when it is a positive number.
pub fn read_watcher_opts_from_env() -> WatcherOptions {
    let ms = js::number(
        std::env::var("OBSIDIAN_BRAIN_WATCH_DEBOUNCE_MS")
            .ok()
            .as_deref(),
    );
    WatcherOptions {
        debounce_ms: (ms.is_finite() && ms > 0.0).then_some(ms as u64),
        stability_ms: None,
    }
}

static IGNORED_DIR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|/)(\.obsidian|\.trash|\.git|node_modules|attachments)(/|$)")
        .expect("valid pattern")
});
static HAS_EXTENSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\.[A-Za-z0-9]+$").expect("valid pattern"));
static MD_EXTENSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\.md$").expect("valid pattern"));

fn vault_relative(roots: &[PathBuf], path: &Path) -> String {
    let rel = roots
        .iter()
        .find_map(|r| path.strip_prefix(r).ok())
        .unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

/// A directory under one of the ignored names. The path is taken relative
/// to the vault, so a vault that itself lives under, say, a `.git`
/// directory still works.
fn is_ignored_dir(roots: &[PathBuf], path: &Path) -> bool {
    IGNORED_DIR.is_match(&vault_relative(roots, path))
}

/// A file in an ignored directory, or a file with an extension other than
/// `.md`.
fn is_ignored_file(roots: &[PathBuf], path: &Path) -> bool {
    let rel = vault_relative(roots, path);
    IGNORED_DIR.is_match(&rel) || (HAS_EXTENSION.is_match(&rel) && !MD_EXTENSION.is_match(&rel))
}

/// A running watcher. [`WatcherHandle::close`] stops it.
pub struct WatcherHandle {
    cancel: CancellationToken,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl WatcherHandle {
    /// Stop watching, drop the pending timers and wait for the reindexes in
    /// flight, so the caller can close the store safely.
    pub async fn close(&self) {
        self.cancel.cancel();
        let task = self.task.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Raw {
    Created,
    Changed,
    Removed,
}

struct Pending {
    deadline: Instant,
    last: Raw,
}

/// Watch `ctx`'s vault and index each changed note with
/// `IndexPipeline::index_single_note`. Must run inside a Tokio runtime.
pub fn start_watcher(
    ctx: Arc<VaultContext>,
    opts: WatcherOptions,
) -> anyhow::Result<WatcherHandle> {
    let vault_path = std::path::absolute(&ctx.config.vault_path)?;
    let canonical = std::fs::canonicalize(&vault_path).unwrap_or_else(|_| vault_path.clone());
    let debounce = Duration::from_millis(opts.debounce_ms.unwrap_or(DEFAULT_DEBOUNCE_MS));
    let stability = Duration::from_millis(opts.stability_ms.unwrap_or(DEFAULT_STABILITY_MS));

    let (tx, rx) = mpsc::unbounded_channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })?;
    let roots = vec![vault_path.clone(), canonical];
    watch_tree(&mut watcher, &roots, &vault_path);
    tracing::info!(vault_path = %vault_path.display(), "watching {} for changes", vault_path.display());

    let cancel = CancellationToken::new();
    let task = tokio::spawn(run(Loop {
        ctx,
        watcher,
        rx,
        roots,
        debounce,
        stability,
        cancel: cancel.clone(),
    }));
    Ok(WatcherHandle {
        cancel,
        task: Mutex::new(Some(task)),
    })
}

/// Watch `dir` and every directory under it that is not ignored, each
/// without recursion, so ignored trees cost no watches.
fn watch_tree(
    watcher: &mut notify::RecommendedWatcher,
    roots: &[PathBuf],
    dir: &Path,
) -> Vec<PathBuf> {
    let mut md_files = Vec::new();
    let walk = walkdir::WalkDir::new(dir)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| !e.file_type().is_dir() || !is_ignored_dir(roots, e.path()));
    for entry in walk.filter_map(Result::ok) {
        if entry.file_type().is_dir() {
            if let Err(err) = watcher.watch(entry.path(), RecursiveMode::NonRecursive) {
                tracing::error!(error = %err, "watcher error: {err}");
            }
        } else if !is_ignored_file(roots, entry.path()) {
            md_files.push(entry.path().to_path_buf());
        }
    }
    md_files
}

struct Loop {
    ctx: Arc<VaultContext>,
    watcher: notify::RecommendedWatcher,
    rx: mpsc::UnboundedReceiver<notify::Result<notify::Event>>,
    roots: Vec<PathBuf>,
    debounce: Duration,
    stability: Duration,
    cancel: CancellationToken,
}

impl Loop {
    fn rel_path(&self, abs: &Path) -> Option<String> {
        let rel = self.roots.iter().find_map(|r| abs.strip_prefix(r).ok())?;
        let rel = rel.to_string_lossy().replace('\\', "/");
        (rel.ends_with(".md") && !rel.starts_with("..") && !rel.is_empty()).then_some(rel)
    }
}

async fn run(mut lp: Loop) {
    let mut pending: HashMap<String, Pending> = HashMap::new();
    let mut in_flight: JoinSet<()> = JoinSet::new();

    loop {
        while in_flight.try_join_next().is_some() {}
        let next = pending.values().map(|p| p.deadline).min();
        tokio::select! {
            _ = lp.cancel.cancelled() => break,
            ev = lp.rx.recv() => match ev {
                None => break,
                Some(Err(err)) => tracing::error!(error = %err, "watcher error: {err}"),
                Some(Ok(event)) => handle_event(&mut lp, &mut pending, event),
            },
            _ = tokio::time::sleep_until(next.unwrap_or_else(Instant::now)), if next.is_some() => {
                let now = Instant::now();
                let due: Vec<String> = pending
                    .iter()
                    .filter(|(_, p)| p.deadline <= now)
                    .map(|(k, _)| k.clone())
                    .collect();
                for rel in due {
                    let Some(p) = pending.remove(&rel) else { continue };
                    in_flight.spawn(reindex(lp.ctx.clone(), rel, p.last));
                }
            }
        }
    }

    // Shutting down: pending timers are dropped, the watches go, and the
    // reindexes in flight finish so the store is not closed under them.
    drop(pending);
    drop(lp.watcher);
    while in_flight.join_next().await.is_some() {}
}

fn handle_event(lp: &mut Loop, pending: &mut HashMap<String, Pending>, event: notify::Event) {
    let raw = match event.kind {
        EventKind::Create(_) => Raw::Created,
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => Raw::Removed,
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => Raw::Created,
        EventKind::Modify(ModifyKind::Name(_)) => Raw::Changed,
        EventKind::Modify(_) => Raw::Changed,
        EventKind::Remove(_) => Raw::Removed,
        EventKind::Access(_) | EventKind::Any | EventKind::Other => return,
    };
    for path in &event.paths {
        // A new directory gets its own watches, and the notes already in it
        // count as added.
        let is_new_dir = matches!(event.kind, EventKind::Create(CreateKind::Folder))
            || (raw == Raw::Created && path.is_dir());
        if is_new_dir {
            if is_ignored_dir(&lp.roots, path) {
                continue;
            }
            let roots = lp.roots.clone();
            for file in watch_tree(&mut lp.watcher, &roots, path) {
                schedule(lp, pending, &file, Raw::Created);
            }
            continue;
        }
        if is_ignored_file(&lp.roots, path) {
            continue;
        }
        schedule(lp, pending, path, raw);
    }
}

fn schedule(lp: &Loop, pending: &mut HashMap<String, Pending>, abs: &Path, raw: Raw) {
    let Some(rel) = lp.rel_path(abs) else {
        return;
    };
    let wait = match raw {
        Raw::Removed => lp.debounce,
        Raw::Created | Raw::Changed => lp.stability + lp.debounce,
    };
    pending.insert(
        rel,
        Pending {
            deadline: Instant::now() + wait,
            last: raw,
        },
    );
}

async fn reindex(ctx: Arc<VaultContext>, rel: String, last: Raw) {
    let vault_path = ctx.config.vault_path.clone();
    let exists = vault_path.join(&rel).is_file();
    let event = match (exists, last) {
        (false, _) => NoteEvent::Unlink,
        (true, Raw::Created) => NoteEvent::Add,
        (true, _) => NoteEvent::Change,
    };
    let outcome = async {
        ctx.ensure_embedder_ready().await?;
        ctx.pipeline
            .index_single_note(&vault_path, &rel, event)
            .await
    }
    .await;
    match outcome {
        Ok(result) if result.indexed || result.deleted => {
            let verb = if result.deleted {
                "removed"
            } else {
                event.as_str()
            };
            let stubs = if result.stubs_created > 0 {
                format!(" (+{} stubs)", result.stubs_created)
            } else {
                String::new()
            };
            tracing::info!(
                event = verb,
                path = %rel,
                stubs_created = result.stubs_created,
                "{verb} {rel}{stubs}"
            );
        }
        Ok(_) => {}
        Err(err) => {
            tracing::error!(path = %rel, error = %err, "reindex failed for {rel}: {err}");
        }
    }
}
