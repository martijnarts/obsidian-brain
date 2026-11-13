//! The graph group: `find_connections`, `find_path_between`, `rank_notes`,
//! `reindex`, `index_status`, `read_canvas`, `edit_canvas`, `list_tasks`,
//! `set_task_status` and `ensure_block_id`.

mod canvas;
mod ensure_block_id;
mod find_connections;
mod find_path_between;
mod index_status;
mod list_tasks;
mod rank_notes;
mod reindex;
mod set_task_status;

use std::path::Path;
use std::sync::Arc;

use obsidian_brain::context::VaultContext;
use obsidian_brain::store::nodes::upsert_node;
use serde_json::Value;

use crate::support::{FakeEmbedder, parsed_node};
use crate::support::{err, ids, ok};

/// A context over `vault` with an in-memory index and an instant embedder.
fn context(vault: &Path) -> Arc<VaultContext> {
    crate::support::context(vault, Arc::new(FakeEmbedder::instant()))
}

fn node(ctx: &VaultContext, id: &str, title: &str, frontmatter: Value) {
    let node = parsed_node(id, title, "", frontmatter);
    ctx.db.with(|c| upsert_node(c, &node)).unwrap();
}

fn edge(ctx: &VaultContext, source: &str, target: &str) {
    crate::support::edge(ctx, source, target, "");
}
