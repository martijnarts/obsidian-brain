//! `create_folder`: make a folder and any missing parents. An existing
//! folder is not an error (`created: false`); an existing file at the path
//! is. An empty folder holds no notes, so the index has nothing to update.

use anyhow::{anyhow, bail};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::mcp::tool::ToolSpec;
use crate::util::errors::node_fs_error_message;
use crate::vault::vault_path::resolve_vault_path;

const DESCRIPTION: &str = "Create a folder in the vault, with any missing parent folders. Succeeds with `created: false` when the folder already exists.";

#[derive(Debug, Deserialize, JsonSchema)]
struct Args {
    /// Vault-relative folder path, e.g. `Projects/2026`.
    path: String,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("create_folder", DESCRIPTION, |ctx, args| async move {
        let resolved = resolve_vault_path(&ctx.config.vault_path, &args.path)?;
        let (rel, abs) = (resolved.rel, resolved.abs);
        if rel.is_empty() {
            bail!("Path is the vault root, which already exists");
        }

        match std::fs::metadata(&abs) {
            Ok(st) if st.is_dir() => return Ok(json!({ "path": rel, "created": false })),
            Ok(_) => bail!("A file already exists at {rel}"),
            Err(_) => {}
        }

        std::fs::create_dir_all(&abs)
            .map_err(|e| anyhow!(node_fs_error_message(&e, "mkdir", &abs, None)))?;
        Ok(json!({ "path": rel, "created": true }))
    })
}
