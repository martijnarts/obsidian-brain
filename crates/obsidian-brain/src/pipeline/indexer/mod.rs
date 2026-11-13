//! The incremental indexer: parse the vault, embed what changed, and keep
//! nodes, chunks, vectors, edges, stubs and sync state in step.
//!
//! Writes go in one transaction per note, on a blocking thread, and the
//! store lock is released between notes and while the embedder runs, so
//! tool reads interleave with a long pass.

mod embedder_errors;
mod fallback_text;
mod self_heal;
mod stubs;
mod types;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::UNIX_EPOCH;

use anyhow::{Result, anyhow};
use regex::Regex;
use rusqlite::Transaction;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub use embedder_errors::{is_dead_embedder_error, is_too_long_error};
pub use fallback_text::{MIN_FALLBACK_CHARS, build_title_fallback_text};
pub use self_heal::{RepairCounts, repair_missing_embeddings};
pub use stubs::{materialise_stubs, resolve_forward_stubs};
pub use types::{IndexStats, NoteEvent, SingleNoteResult};

use crate::embeddings::capacity::{
    EmbedderCapacity, cached_capacity, probe_capacity, record_failed_chunk,
    reduce_discovered_max_tokens, reset_discovered_capacity, store_capacity,
};
use crate::embeddings::chunker::{
    ChunkerConfig, DEFAULT_CHUNKER_CONFIG, build_chunk_embedding_text, chunk_markdown,
};
use crate::embeddings::embedder::TransformersEmbedder;
use crate::embeddings::{Embedder, TaskType};
use crate::store::Db;
use crate::store::chunks::{
    chunk_id, delete_chunks, get_chunk, get_chunk_ids_for_node, upsert_chunk_row,
    upsert_chunk_vector,
};
use crate::store::edges::{delete_edges_by_source, insert_edge};
use crate::store::embeddings::upsert_embedding;
use crate::store::nodes::{
    delete_node, get_node, migrate_stub_to_real, prune_all_orphan_stubs, upsert_node,
};
use crate::store::sync::{get_all_sync_paths, get_sync_mtime, set_sync_mtime};
use crate::types::{Chunk, ParsedEdge, ParsedNode};
use crate::util::errors::{error_message, node_fs_error_message};
use crate::util::js;
use crate::vault::parser::{parse_single_file, parse_vault};

static SQL_ERROR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)too (few|many) parameter values|stmt\.run|prepared statement|no such (table|column)",
    )
    .expect("valid pattern")
});

struct State {
    chunker_config: ChunkerConfig,
    capacity: Option<EmbedderCapacity>,
}

/// Indexes one vault into one store. Cheap to share behind an `Arc`.
pub struct IndexPipeline {
    db: Db,
    embedder: Arc<dyn Embedder>,
    state: Mutex<State>,
}

/// A chunk to write, with the outcome of its embed call.
enum ChunkWork {
    /// Same content hash as the stored row: nothing to do.
    Unchanged,
    Embedded(Chunk, Vec<f32>),
    Failed {
        chunk: Chunk,
        reason: &'static str,
        message: String,
    },
}

/// Everything one note's transaction writes.
struct NoteWrite {
    node: ParsedNode,
    edges: Vec<ParsedEdge>,
    mtime: f64,
    /// `None` when the note has no embeddable content at all.
    chunks: Option<Vec<ChunkWork>>,
    used_fallback: bool,
    fresh_ids: Vec<String>,
    note_vector: std::result::Result<Vec<f32>, (&'static str, String)>,
}

/// The modification time in milliseconds, fractional, as `stat().mtimeMs`.
fn mtime_ms(abs: &Path) -> Result<f64> {
    let meta = std::fs::metadata(abs)
        .map_err(|e| anyhow!(node_fs_error_message(&e, "stat", abs, None)))?;
    let modified = meta
        .modified()
        .map_err(|e| anyhow!(node_fs_error_message(&e, "stat", abs, None)))?;
    Ok(match modified.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64() * 1000.0,
        Err(e) => -(e.duration().as_secs_f64() * 1000.0),
    })
}

fn sha256_hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

impl IndexPipeline {
    pub fn new(db: Db, embedder: Arc<dyn Embedder>) -> Self {
        Self::with_chunker_config(db, embedder, DEFAULT_CHUNKER_CONFIG)
    }

    /// A pipeline whose chunker options other than `chunk_size` stay as
    /// given. `chunk_size` follows the embedder's capacity.
    pub fn with_chunker_config(db: Db, embedder: Arc<dyn Embedder>, config: ChunkerConfig) -> Self {
        Self {
            db,
            embedder,
            state: Mutex::new(State {
                chunker_config: config,
                capacity: None,
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The chunker config in effect.
    pub fn chunker_config(&self) -> ChunkerConfig {
        self.state().chunker_config
    }

    /// Run `f` in one transaction on a blocking thread.
    async fn tx<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>) -> Result<T> + Send + 'static,
    {
        let db = self.db.clone();
        match tokio::task::spawn_blocking(move || db.with_tx(f)).await {
            Ok(r) => r,
            Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
            Err(e) => Err(anyhow!("index task cancelled: {e}")),
        }
    }

    /// Fetch (or fetch again) the embedder's token capacity and take the
    /// chunk size from its budget. Every other chunker option stays.
    /// Runs on the first index call; call it again after a model swap.
    pub async fn refresh_capacity(&self) -> Result<()> {
        let embedder = self.embedder.clone();
        let cached = self
            .db
            .run(move |c| cached_capacity(c, &*embedder, &js::process_env()))
            .await?;
        let capacity = match cached {
            Some(c) => c,
            None => {
                let probed = probe_capacity(&*self.embedder).await;
                let embedder = self.embedder.clone();
                self.db
                    .run(move |c| store_capacity(c, &*embedder, probed))
                    .await?
            }
        };
        let mut st = self.state();
        st.capacity = Some(capacity);
        st.chunker_config.chunk_size = capacity.chunk_budget_chars as usize;
        Ok(())
    }

    async fn ensure_capacity(&self) -> Result<()> {
        if self.state().capacity.is_none() {
            self.refresh_capacity().await?;
        }
        Ok(())
    }

    /// Index the whole vault, incrementally: files whose mtime has not
    /// advanced are skipped, deleted files are removed.
    pub async fn index(&self, vault_path: &Path) -> Result<IndexStats> {
        self.ensure_capacity().await?;

        // Drop any drift in `discovered_max_tokens` from earlier runs, so one
        // freak chunk cannot shrink the budget of every later note.
        let embedder = self.embedder.clone();
        self.db
            .run(move |c| reset_discovered_capacity(c, &*embedder))
            .await?;
        self.refresh_capacity().await?;

        match self.index_inner(vault_path).await {
            Ok(stats) => Ok(stats),
            Err(err) => {
                let msg = error_message(&err);
                if SQL_ERROR.is_match(&msg) {
                    let snippet = js::utf16_slice(&msg, 0, 300);
                    tracing::error!(
                        error_snippet = %snippet,
                        "SQL error during reindex — likely schema drift or stale npx cache. Run: rm -rf ~/.npm/_npx && relaunch. Error: {snippet}"
                    );
                    return Err(anyhow!(
                        "reindex failed: SQL error (likely schema drift or stale install) — {msg}"
                    ));
                }
                Err(err)
            }
        }
    }

    async fn index_inner(&self, vault_path: &Path) -> Result<IndexStats> {
        let mut stats = IndexStats::default();

        let root = vault_path.to_path_buf();
        let (parsed, mtimes) = tokio::task::spawn_blocking(move || -> Result<_> {
            let parsed = parse_vault(&root)?;
            let mtimes = parsed
                .nodes
                .iter()
                .map(|n| mtime_ms(&root.join(&n.id)))
                .collect::<Result<Vec<f64>>>()?;
            Ok((parsed, mtimes))
        })
        .await
        .map_err(|e| anyhow!("vault scan failed: {e}"))??;

        // Remove the files that are gone.
        let current: Vec<String> = parsed.nodes.iter().map(|n| n.id.clone()).collect();
        self.tx(move |tx| {
            let current: std::collections::HashSet<&str> =
                current.iter().map(String::as_str).collect();
            for old in get_all_sync_paths(tx)? {
                if !current.contains(old.as_str()) {
                    delete_node(tx, &old)?;
                }
            }
            Ok(())
        })
        .await?;

        let mut edges_by_source: HashMap<String, Vec<ParsedEdge>> = HashMap::new();
        for edge in parsed.edges {
            edges_by_source
                .entry(edge.source_id.clone())
                .or_default()
                .push(edge);
        }
        for (node, mtime) in parsed.nodes.into_iter().zip(mtimes) {
            let edges = edges_by_source.remove(&node.id).unwrap_or_default();
            self.apply_node(node, edges, mtime, &mut stats).await?;
        }

        // Materialise the stubs, then resolve forward references: a stub
        // whose bare stem now matches a real note has its inbound edges
        // repointed and goes. Stubs left without inbound edges are pruned.
        let stub_ids = parsed.stub_ids;
        stats.stub_nodes_created += self
            .tx(move |tx| {
                let created = materialise_stubs(tx, &stub_ids)?;
                resolve_forward_stubs(tx)?;
                prune_all_orphan_stubs(tx)?;
                Ok(created)
            })
            .await?;

        // A summary only when chunks were skipped, so the happy path stays quiet.
        if stats.chunks_skipped > 0 {
            tracing::info!(
                nodes_indexed = stats.nodes_indexed,
                chunks_ok = stats.chunks_ok,
                chunks_skipped = stats.chunks_skipped,
                "indexed {} notes ({} chunks ok, {} chunks skipped).",
                stats.nodes_indexed,
                stats.chunks_ok,
                stats.chunks_skipped
            );
        }

        let counts = self.tx(|tx| Ok(repair_missing_embeddings(tx)?)).await?;
        stats.notes_missing_embeddings = counts.unexpected_missing;
        stats.notes_no_embeddable_content = counts.no_content_total;
        Ok(stats)
    }

    /// Reindex one file. The watcher calls it on every debounced change.
    pub async fn index_single_note(
        &self,
        vault_path: &Path,
        rel_path: &str,
        event: NoteEvent,
    ) -> Result<SingleNoteResult> {
        self.ensure_capacity().await?;

        if event == NoteEvent::Unlink {
            let rel = rel_path.to_string();
            let existed = self
                .tx(move |tx| {
                    let existed = get_node(tx, &rel)?.is_some();
                    delete_node(tx, &rel)?;
                    Ok(existed)
                })
                .await?;
            return Ok(SingleNoteResult {
                deleted: existed,
                ..SingleNoteResult::default()
            });
        }

        let mtime = mtime_ms(&vault_path.join(rel_path))?;

        // The sync table is the best-effort list of the files the indexer
        // knows, good enough for wiki-link resolution. A new file joins it.
        let mut all_paths = self.db.run(get_all_sync_paths).await?;
        if !all_paths.iter().any(|p| p == rel_path) {
            all_paths.push(rel_path.to_string());
        }
        let root: PathBuf = vault_path.to_path_buf();
        let rel = rel_path.to_string();
        let parsed =
            tokio::task::spawn_blocking(move || parse_single_file(&root, &rel, &all_paths))
                .await
                .map_err(|e| anyhow!("note parse failed: {e}"))??;

        let mut stats = IndexStats::default();
        self.apply_node(parsed.node, parsed.edges, mtime, &mut stats)
            .await?;

        // Resolve a forward-reference stub for this note's bare stem, as the
        // full pass does. Without it, edges would sit on stub targets
        // forever and a later `move_note` would miss them.
        let stub_ids = parsed.stub_ids;
        let rel = rel_path.to_string();
        let stem = {
            let base = rel.rsplit('/').next().unwrap_or(&rel);
            match base.strip_suffix(".md") {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => base.to_string(),
            }
        };
        stats.stub_nodes_created += self
            .tx(move |tx| {
                let created = materialise_stubs(tx, &stub_ids)?;
                if !stem.is_empty() {
                    migrate_stub_to_real(tx, &format!("_stub/{stem}.md"), &rel)?;
                }
                Ok(created)
            })
            .await?;

        Ok(SingleNoteResult {
            indexed: stats.nodes_indexed > 0,
            skipped: stats.nodes_skipped > 0,
            deleted: false,
            edges_indexed: stats.edges_indexed,
            stubs_created: stats.stub_nodes_created,
        })
    }

    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        self.embedder.embed(text, TaskType::Document).await
    }

    /// Index one parsed note unless its mtime has not advanced.
    ///
    /// Chunks are content-addressed: a chunk whose hash matches the stored
    /// row keeps its vector without an embed call. An embed error that
    /// means "too long" (or any unknown error) skips the chunk; one that
    /// means the embedder is gone aborts the pass. A note with nothing
    /// chunkable gets one fallback chunk from its title, tags, scalar
    /// frontmatter and first link targets. Chunks that no longer exist are
    /// dropped. The note-level vector in `nodes_vec` is kept for callers of
    /// the note-level search.
    async fn apply_node(
        &self,
        node: ParsedNode,
        edges: Vec<ParsedEdge>,
        mtime: f64,
        stats: &mut IndexStats,
    ) -> Result<()> {
        let mut chunks = chunk_markdown(&node.content, &self.chunker_config());
        let mut used_fallback = false;
        let mut no_content = false;
        if chunks.is_empty() {
            let text = build_title_fallback_text(&node);
            if js::utf16_len(js::trim(&text)) < MIN_FALLBACK_CHARS {
                no_content = true;
            } else {
                used_fallback = true;
                chunks.push(Chunk {
                    chunk_index: 0,
                    heading: (!node.title.is_empty()).then(|| node.title.clone()),
                    heading_level: None,
                    content_hash: sha256_hex(&text),
                    content: text,
                    start_line: 0,
                    end_line: 0,
                });
            }
        }

        let node_id = node.id.clone();
        let ids: Vec<String> = chunks
            .iter()
            .map(|c| chunk_id(&node_id, c.chunk_index))
            .collect();
        let lookup_ids = ids.clone();
        let id_for_read = node_id.clone();
        let (prev_mtime, stored_hashes) = self
            .db
            .run(move |c| -> Result<_> {
                let prev = get_sync_mtime(c, &id_for_read)?;
                let hashes = lookup_ids
                    .iter()
                    .map(|id| Ok(get_chunk(c, id)?.map(|row| row.content_hash)))
                    .collect::<Result<Vec<Option<String>>>>()?;
                Ok((prev, hashes))
            })
            .await?;
        if prev_mtime.is_some_and(|p| p >= mtime) {
            stats.nodes_skipped += 1;
            return Ok(());
        }

        let mut work = Vec::with_capacity(chunks.len());
        if !no_content {
            for (chunk, stored) in chunks.into_iter().zip(stored_hashes) {
                if stored.as_deref() == Some(chunk.content_hash.as_str()) {
                    // Biggest win on a repeated index of an unchanged note.
                    stats.chunks_ok += 1;
                    work.push(ChunkWork::Unchanged);
                    continue;
                }
                match self.embed(&build_chunk_embedding_text(&chunk)).await {
                    Ok(vec) => {
                        stats.chunks_ok += 1;
                        work.push(ChunkWork::Embedded(chunk, vec));
                    }
                    Err(err) => {
                        let msg = error_message(&err);
                        if is_dead_embedder_error(&msg) {
                            return Err(err);
                        }
                        let chars = js::utf16_len(&chunk.content);
                        let reason = if is_too_long_error(&msg) {
                            tracing::warn!(
                                node_id = %node_id,
                                chunk_index = chunk.chunk_index,
                                chars,
                                reason = "too-long",
                                "chunk too large for embedder — skipping (node: {node_id}, chunk: {}, chars: {chars})",
                                chunk.chunk_index
                            );
                            "too-long"
                        } else {
                            // Better to skip than to halt.
                            tracing::warn!(
                                node_id = %node_id,
                                chunk_index = chunk.chunk_index,
                                chars,
                                reason = "embed-error",
                                error = %msg,
                                "chunk embed failed with unrecognised error — skipping (node: {node_id}, chunk: {}, chars: {chars}): {msg}",
                                chunk.chunk_index
                            );
                            "embed-error"
                        };
                        stats.chunks_skipped += 1;
                        work.push(ChunkWork::Failed {
                            chunk,
                            reason,
                            message: js::utf16_slice(&msg, 0, 500),
                        });
                    }
                }
            }
        }
        if no_content {
            stats.notes_no_embeddable_content += 1;
        }

        let tags: Vec<String> = match node.frontmatter.get("tags") {
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| match v {
                    Value::Null => String::new(),
                    other => js::to_js_string(other),
                })
                .collect(),
            _ => Vec::new(),
        };
        let note_text =
            TransformersEmbedder::build_embedding_text(&node.title, &tags, &node.content);
        let note_vector = match self.embed(&note_text).await.and_then(|v| {
            if v.is_empty() {
                Err(anyhow!("embedder returned empty/invalid vector for note"))
            } else {
                Ok(v)
            }
        }) {
            Ok(v) => Ok(v),
            Err(err) => {
                let msg = error_message(&err);
                if is_dead_embedder_error(&msg) {
                    return Err(err);
                }
                let chars = js::utf16_len(&note_text);
                let reason = js::utf16_slice(&msg, 0, 200);
                tracing::warn!(
                    node_id = %node_id,
                    chars,
                    reason = %reason,
                    "note-level embedding failed — skipping (node: {node_id}, chars: {chars}, reason: {reason})"
                );
                // Chunk vectors drive semantic search; a missing note-level
                // vector is not fatal.
                let kind = if is_too_long_error(&msg) {
                    "note-too-long"
                } else {
                    "note-embed-error"
                };
                Err((kind, js::utf16_slice(&msg, 0, 500)))
            }
        };

        stats.edges_indexed += edges.len();
        stats.nodes_indexed += 1;
        let write = NoteWrite {
            node,
            edges,
            mtime,
            chunks: (!no_content).then_some(work),
            used_fallback,
            fresh_ids: ids,
            note_vector,
        };
        let embedder = self.embedder.clone();
        self.tx(move |tx| write_note(tx, &*embedder, write)).await
    }
}

fn write_note(tx: &Transaction<'_>, embedder: &dyn Embedder, w: NoteWrite) -> Result<()> {
    let node_id = w.node.id.clone();
    upsert_node(tx, &w.node)?;

    match w.chunks {
        None => {
            // Recorded so index_status can show the bucket and the note is
            // not retried on every pass. Stale chunks go (a body that was
            // emptied to whitespace).
            record_failed_chunk(
                tx,
                &format!("{node_id}#no-content"),
                &node_id,
                "no-embeddable-content",
                None,
            )?;
            let previous = get_chunk_ids_for_node(tx, &node_id)?;
            if !previous.is_empty() {
                delete_chunks(tx, &previous)?;
            }
        }
        Some(work) => {
            if w.used_fallback {
                // A note that gained a usable fallback (a title, say) is no
                // longer "no content".
                tx.execute(
                    "DELETE FROM failed_chunks WHERE chunk_id = ?",
                    [format!("{node_id}#no-content")],
                )?;
            }
            for item in work {
                match item {
                    ChunkWork::Unchanged => {}
                    ChunkWork::Embedded(chunk, vec) => {
                        let rowid = upsert_chunk_row(tx, &node_id, &chunk)?;
                        upsert_chunk_vector(tx, rowid, &vec)?;
                    }
                    ChunkWork::Failed {
                        chunk,
                        reason,
                        message,
                    } => {
                        upsert_chunk_row(tx, &node_id, &chunk)?;
                        // Persisted so later passes and the capacity logic can
                        // see the failure distribution.
                        record_failed_chunk(
                            tx,
                            &chunk_id(&node_id, chunk.chunk_index),
                            &node_id,
                            reason,
                            Some(&message),
                        )?;
                        // Shrink the discovered token ceiling so the next pass
                        // uses smaller chunks, at the capacity module's chars
                        // per token.
                        let approx_tokens =
                            (js::utf16_len(&chunk.content) as f64 / 2.5).ceil() as u64;
                        reduce_discovered_max_tokens(tx, embedder, approx_tokens)?;
                    }
                }
            }
            // Drop the chunks that no longer exist (the note got shorter, a
            // heading changed).
            let stale: Vec<String> = get_chunk_ids_for_node(tx, &node_id)?
                .into_iter()
                .filter(|id| !w.fresh_ids.contains(id))
                .collect();
            if !stale.is_empty() {
                delete_chunks(tx, &stale)?;
            }
        }
    }

    match &w.note_vector {
        Ok(vec) => upsert_embedding(tx, &node_id, vec)?,
        Err((reason, message)) => record_failed_chunk(
            tx,
            &format!("{node_id}#note"),
            &node_id,
            reason,
            Some(message),
        )?,
    }

    delete_edges_by_source(tx, &node_id)?;
    for edge in &w.edges {
        insert_edge(tx, edge)?;
    }
    set_sync_mtime(tx, &node_id, w.mtime, None)?;
    Ok(())
}
