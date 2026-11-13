//! Semantic, full-text and hybrid search over the index.

pub mod unified;

pub use unified::{ChunkAwareResult, Search, SearchUnique, reciprocal_rank_fusion};
