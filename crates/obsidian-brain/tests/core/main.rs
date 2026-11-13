//! Tests of the core: the vault writer and mover, the pipeline, search,
//! the vault context and the vault set. Ported from `test/pipeline/`,
//! `test/search/`, `test/vault/{writer,mover}.test.ts`,
//! `test/context.test.ts`, `test/vaults.test.ts`,
//! `test/shared-embedder.test.ts` and `test/integration.test.ts`.

mod bootstrap;
mod context;
mod empty_note_fallback;
mod fault_tolerance;
mod indexer;
mod integration;
mod mover;
mod reindex_recovery;
mod search;
mod support;
mod vaults;
mod watcher;
mod writer;
