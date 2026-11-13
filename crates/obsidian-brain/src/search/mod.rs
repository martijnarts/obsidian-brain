//! Ported from `src/search` of the TypeScript server.

pub mod unified;

pub use unified::{ChunkAwareResult, Search, SearchUnique, reciprocal_rank_fusion};
