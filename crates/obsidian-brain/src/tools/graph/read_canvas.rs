//! `read_canvas`: the nodes and edges of a `.canvas` file as stored,
//! including fields this server does not model.

use anyhow::anyhow;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::mcp::tool::ToolSpec;
use crate::vault::canvas::{canvas_path, read_canvas};

const DESCRIPTION: &str = "Read an Obsidian canvas (JSON Canvas) and return its nodes and edges with their ids, positions and content.";

#[derive(Debug, Deserialize, JsonSchema)]
struct Args {
    /// Vault-relative path of the `.canvas` file. The extension is optional.
    #[schemars(length(min = 1))]
    path: String,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("read_canvas", DESCRIPTION, |ctx, args| async move {
        let path = canvas_path(&args.path);
        let doc = read_canvas(&ctx.config.vault_path, &path)?
            .ok_or_else(|| anyhow!("Canvas not found: {path}"))?;
        Ok(json!({ "path": path, "nodes": doc.nodes, "edges": doc.edges }))
    })
}
