//! Ported from `src/embeddings` of the TypeScript server: the embedders, the
//! markdown chunker, and the model metadata and capacity they rely on.

pub mod capacity;
pub mod chunker;
pub mod embedder;
pub mod errors;
pub mod factory;
pub mod metadata_cache;
pub mod metadata_resolver;
pub mod ollama;
pub mod overrides;
pub mod presets;
pub mod seed_loader;
pub mod types;
pub mod user_config;

pub use errors::{EmbedderLoadError, EmbedderLoadErrorKind};
pub use factory::{SharedEmbedder, create_embedder, model_cache_dir, share_embedder};
pub use types::{Embedder, EmbedderMetadata, PrefixSource, TaskType};
