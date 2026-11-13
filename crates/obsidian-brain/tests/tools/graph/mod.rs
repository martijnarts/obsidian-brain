//! The graph group: `find_connections`, `find_path_between`, `rank_notes`,
//! `reindex`, `index_status`, `read_canvas`, `edit_canvas`, `list_tasks`,
//! `set_task_status` and `ensure_block_id`.

#[allow(dead_code)]
#[path = "../../core/support.rs"]
mod support;

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
use obsidian_brain::store::edges::insert_edge;
use obsidian_brain::store::nodes::upsert_node;
use obsidian_brain::types::{ParsedEdge, ParsedNode};
use serde_json::Value;

use support::FakeEmbedder;

/// A context over `vault` with an in-memory index and an instant embedder.
fn context(vault: &Path) -> Arc<VaultContext> {
    support::context(vault, Arc::new(FakeEmbedder::instant()))
}

/// Call a tool the way the MCP layer does. `Err` holds the error text.
async fn call(ctx: &Arc<VaultContext>, name: &str, args: Value) -> Result<Value, String> {
    let spec = obsidian_brain::tools::all()
        .into_iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("tool not registered: {name}"));
    let Value::Object(args) = args else {
        panic!("arguments must be an object")
    };
    spec.call(ctx.clone(), args)
        .await
        .map_err(|e| format!("{e:#}"))
}

/// [`call`] that must succeed.
async fn ok(ctx: &Arc<VaultContext>, name: &str, args: Value) -> Value {
    match call(ctx, name, args).await {
        Ok(v) => v,
        Err(e) => panic!("{name} failed: {e}"),
    }
}

/// [`call`] that must fail; returns the error text.
async fn err(ctx: &Arc<VaultContext>, name: &str, args: Value) -> String {
    match call(ctx, name, args).await {
        Ok(v) => panic!("{name} succeeded: {v}"),
        Err(e) => e,
    }
}

fn node(ctx: &VaultContext, id: &str, title: &str, frontmatter: Value) {
    ctx.db.with(|c| {
        upsert_node(
            c,
            &ParsedNode {
                id: id.into(),
                title: title.into(),
                content: String::new(),
                frontmatter: frontmatter.as_object().unwrap().clone(),
            },
        )
        .unwrap()
    });
}

fn edge(ctx: &VaultContext, source: &str, target: &str) {
    ctx.db.with(|c| {
        insert_edge(
            c,
            &ParsedEdge {
                source_id: source.into(),
                target_id: target.into(),
                context: String::new(),
                target_subpath: None,
            },
        )
        .unwrap()
    });
}

fn ids(list: &Value) -> Vec<&str> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap())
        .collect()
}
