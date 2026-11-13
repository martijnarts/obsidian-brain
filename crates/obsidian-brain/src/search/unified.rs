//! Semantic, full-text and hybrid search over one vault's index.

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;

use crate::embeddings::{Embedder, TaskType};
use crate::store::Db;
use crate::store::chunks::{ChunkSearchHit, count_chunks, search_chunk_vectors};
use crate::store::embeddings::search_vector;
use crate::store::fulltext::search_full_text;
use crate::types::SearchResult;
use crate::util::js;

/// A search result, with the fields of the chunk it came from when it came
/// from one. A chunk field that is `None` is left out of the JSON; an
/// inner `None` is written as `null`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkAwareResult {
    pub node_id: String,
    pub title: String,
    pub score: f64,
    pub excerpt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_heading: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_start_line: Option<Option<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_end_line: Option<Option<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_excerpt: Option<String>,
}

impl From<SearchResult> for ChunkAwareResult {
    fn from(r: SearchResult) -> Self {
        Self {
            node_id: r.node_id,
            title: r.title,
            score: r.score,
            excerpt: r.excerpt,
            chunk_id: None,
            chunk_heading: None,
            chunk_start_line: None,
            chunk_end_line: None,
            chunk_excerpt: None,
        }
    }
}

/// One row per note, or one row per matching chunk.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SearchUnique {
    #[default]
    Notes,
    Chunks,
}

/// Search over one vault. Cheap to clone.
#[derive(Clone)]
pub struct Search {
    db: Db,
    embedder: Arc<dyn Embedder>,
}

fn by_score_desc(a: f64, b: f64) -> std::cmp::Ordering {
    b.partial_cmp(&a).unwrap_or(std::cmp::Ordering::Equal)
}

impl Search {
    pub fn new(db: Db, embedder: Arc<dyn Embedder>) -> Self {
        Self { db, embedder }
    }

    /// Semantic search over `chunks_vec`: the best chunk per note wins, one
    /// row per note. Falls back to the note-level `nodes_vec` when the
    /// chunks table is empty (a new index before its first run).
    pub async fn semantic(
        &self,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<ChunkAwareResult>> {
        if self.db.run(count_chunks).await? == 0 {
            let q = self.embedder.embed(query, TaskType::Query).await?;
            let hits = self.db.run(move |c| search_vector(c, &q, limit)).await?;
            return Ok(hits.into_iter().map(Into::into).collect());
        }
        self.semantic_chunks(query, limit, SearchUnique::Notes)
            .await
    }

    /// Chunk-aware semantic search. Over-fetches raw chunk hits, then keeps
    /// the best one per note (`Notes`) or the raw chunk rows (`Chunks`), so
    /// a caller can see the exact heading and line span that matched.
    pub async fn semantic_chunks(
        &self,
        query: &str,
        limit: usize,
        unique: SearchUnique,
    ) -> anyhow::Result<Vec<ChunkAwareResult>> {
        let q = self.embedder.embed(query, TaskType::Query).await?;
        // Over-fetch so one-per-note still returns about `limit` rows.
        let raw = self
            .db
            .run(move |c| search_chunk_vectors(c, &q, limit * 4))
            .await?;
        if unique == SearchUnique::Chunks {
            return Ok(raw.iter().take(limit).map(to_chunk_aware_result).collect());
        }
        let mut order: Vec<String> = Vec::new();
        let mut by_node: HashMap<String, ChunkSearchHit> = HashMap::new();
        for hit in raw {
            match by_node.get(&hit.node_id) {
                Some(prev) if hit.score <= prev.score => {}
                Some(_) => {
                    by_node.insert(hit.node_id.clone(), hit);
                }
                None => {
                    order.push(hit.node_id.clone());
                    by_node.insert(hit.node_id.clone(), hit);
                }
            }
        }
        let mut best: Vec<ChunkSearchHit> = order
            .into_iter()
            .filter_map(|id| by_node.remove(&id))
            .collect();
        best.sort_by(|a, b| by_score_desc(a.score, b.score));
        Ok(best.iter().take(limit).map(to_chunk_aware_result).collect())
    }

    /// FTS5 search across titles and content.
    pub fn fulltext(&self, query: &str, limit: usize) -> anyhow::Result<Vec<SearchResult>> {
        Ok(self.db.with(|c| search_full_text(c, query, limit))?)
    }

    /// Hybrid retrieval: semantic and full-text, fused by Reciprocal Rank
    /// Fusion. With `Chunks`, each row also carries the fields of the
    /// note's best semantic chunk; with `Notes` the chunk fields are absent.
    pub async fn hybrid(
        &self,
        query: &str,
        limit: usize,
        unique: SearchUnique,
    ) -> anyhow::Result<Vec<ChunkAwareResult>> {
        let fts: Vec<ChunkAwareResult> = self
            .fulltext(query, limit * 2)?
            .into_iter()
            .map(Into::into)
            .collect();

        if unique == SearchUnique::Chunks {
            let sem_chunks = self
                .semantic_chunks(query, limit * 2, SearchUnique::Chunks)
                .await?;
            let mut order: Vec<String> = Vec::new();
            let mut best: HashMap<String, ChunkAwareResult> = HashMap::new();
            for hit in sem_chunks {
                match best.get(&hit.node_id) {
                    Some(prev) if hit.score <= prev.score => {}
                    Some(_) => {
                        best.insert(hit.node_id.clone(), hit);
                    }
                    None => {
                        order.push(hit.node_id.clone());
                        best.insert(hit.node_id.clone(), hit);
                    }
                }
            }
            let mut sem_note_level: Vec<ChunkAwareResult> = order
                .iter()
                .filter_map(|id| best.get(id).cloned())
                .collect();
            sem_note_level.sort_by(|a, b| by_score_desc(a.score, b.score));

            let lists = [sem_note_level, fts];
            let fused = reciprocal_rank_fusion(&lists, |r| r.node_id.clone(), 60.0);
            return Ok(fused
                .into_iter()
                .take(limit)
                .map(|(item, score)| {
                    let b = best.get(&item.node_id);
                    ChunkAwareResult {
                        node_id: item.node_id.clone(),
                        title: item.title.clone(),
                        score,
                        excerpt: item.excerpt.clone(),
                        chunk_id: b.and_then(|b| b.chunk_id.clone()),
                        chunk_heading: b.and_then(|b| b.chunk_heading.clone()),
                        chunk_start_line: b.and_then(|b| b.chunk_start_line),
                        chunk_end_line: b.and_then(|b| b.chunk_end_line),
                        chunk_excerpt: b.and_then(|b| b.chunk_excerpt.clone()),
                    }
                })
                .collect());
        }

        let sem = self.semantic(query, limit * 2).await?;
        let lists = [sem, fts];
        let fused = reciprocal_rank_fusion(&lists, |r| r.node_id.clone(), 60.0);
        Ok(fused
            .into_iter()
            .take(limit)
            .map(|(item, score)| ChunkAwareResult {
                node_id: item.node_id.clone(),
                title: item.title.clone(),
                score,
                excerpt: item.excerpt.clone(),
                chunk_id: None,
                chunk_heading: None,
                chunk_start_line: None,
                chunk_end_line: None,
                chunk_excerpt: None,
            })
            .collect())
    }
}

fn to_chunk_aware_result(hit: &ChunkSearchHit) -> ChunkAwareResult {
    ChunkAwareResult {
        node_id: hit.node_id.clone(),
        title: hit.title.clone(),
        score: hit.score,
        excerpt: excerpt(&hit.content, 200),
        chunk_id: Some(hit.chunk_id.clone()),
        chunk_heading: Some(hit.heading.clone()),
        chunk_start_line: Some(hit.start_line),
        chunk_end_line: Some(hit.end_line),
        chunk_excerpt: Some(excerpt(&hit.content, 200)),
    }
}

/// The trimmed text, cut at `max` UTF-16 units with `...` appended.
fn excerpt(text: &str, max: usize) -> String {
    let trimmed = js::trim(text);
    if js::utf16_len(trimmed) > max {
        format!("{}...", js::utf16_slice(trimmed, 0, max))
    } else {
        trimmed.to_string()
    }
}

/// Reciprocal Rank Fusion across any number of ranked lists.
///
/// An item seen at 0-based rank `r` in a list scores `1 / (k + r + 1)`,
/// summed across lists. `k = 60` is the constant of Cormack, Clarke and
/// Büttcher's 2009 paper. The first time an item is seen fixes which copy
/// is kept. Returns `(item, score)` by descending score; ties keep the
/// order in which items were first seen.
pub fn reciprocal_rank_fusion<'a, T>(
    lists: &'a [Vec<T>],
    key: impl Fn(&T) -> String,
    k: f64,
) -> Vec<(&'a T, f64)> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut agg: Vec<(&'a T, f64)> = Vec::new();
    for list in lists {
        for (idx, item) in list.iter().enumerate() {
            let contrib = 1.0 / (k + idx as f64 + 1.0);
            match index.get(&key(item)) {
                Some(&i) => agg[i].1 += contrib,
                None => {
                    index.insert(key(item), agg.len());
                    agg.push((item, contrib));
                }
            }
        }
    }
    agg.sort_by(|a, b| by_score_desc(a.1, b.1));
    agg
}
