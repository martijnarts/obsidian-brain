//! Ported from `src/pipeline` of the TypeScript server: the indexer, the
//! startup compatibility check and the file watcher.

pub mod bootstrap;
pub mod indexer;
pub mod watcher;

pub use bootstrap::{BootstrapResult, bootstrap};
pub use indexer::{IndexPipeline, IndexStats, NoteEvent, SingleNoteResult};
pub use watcher::{WatcherHandle, WatcherOptions, read_watcher_opts_from_env, start_watcher};
