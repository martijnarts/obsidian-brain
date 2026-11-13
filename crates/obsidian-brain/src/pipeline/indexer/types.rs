//! What an index pass reports.

use serde::Serialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStats {
    pub nodes_indexed: usize,
    pub nodes_skipped: usize,
    pub edges_indexed: usize,
    pub stub_nodes_created: usize,
    pub chunks_ok: usize,
    pub chunks_skipped: usize,
    pub notes_missing_embeddings: usize,
    /// Notes whose body had nothing chunkable and whose title fallback was
    /// empty too, recorded in `failed_chunks` as `no-embeddable-content`.
    /// They are not counted in `notes_missing_embeddings`.
    pub notes_no_embeddable_content: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SingleNoteResult {
    pub indexed: bool,
    pub skipped: bool,
    pub deleted: bool,
    pub edges_indexed: usize,
    pub stubs_created: usize,
}

/// The file event that triggered a single-note reindex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteEvent {
    Add,
    Change,
    Unlink,
}

impl NoteEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Change => "change",
            Self::Unlink => "unlink",
        }
    }
}
