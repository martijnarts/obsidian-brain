//! Vault maintenance tools: broken links, orphans, find/replace, and tag
//! and heading renames.

mod find_broken_links;
mod find_orphaned_notes;
mod rename_heading;
mod rename_tag;
mod search_and_replace;
#[cfg(test)]
mod test_support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use crate::context::VaultContext;
use crate::mcp::tool::{InvalidArguments, ToolSpec};
use crate::util::errors::node_fs_error_message;

pub fn specs() -> Vec<ToolSpec> {
    vec![
        find_broken_links::spec(),
        find_orphaned_notes::spec(),
        search_and_replace::spec(),
        rename_tag::spec(),
        rename_heading::spec(),
    ]
}

/// Run a vault scan on a blocking thread: it reads many files.
async fn blocking<F>(ctx: Arc<VaultContext>, f: F) -> anyhow::Result<Value>
where
    F: FnOnce(&Arc<VaultContext>) -> anyhow::Result<Value> + Send + 'static,
{
    tokio::task::spawn_blocking(move || f(&ctx)).await?
}

/// The check zod's `.min(1)` made on a required string.
fn require_non_empty(field: &str, value: &str) -> anyhow::Result<()> {
    if value.is_empty() {
        return Err(InvalidArguments(format!(
            "{field}: String must contain at least 1 character(s)"
        ))
        .into());
    }
    Ok(())
}

/// `writeFileAtomic`, with the error message Node.js gave the failed step:
/// the temp file write or the rename over the target.
fn write_note(abs: &Path, content: &str) -> Result<(), String> {
    let mut suffix = [0u8; 6];
    getrandom::fill(&mut suffix).map_err(|e| e.to_string())?;
    let mut tmp = abs.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", hex::encode(suffix)));
    let tmp = PathBuf::from(tmp);
    let result = std::fs::write(&tmp, content)
        .map_err(|e| node_fs_error_message(&e, "open", &tmp, None))
        .and_then(|()| {
            std::fs::rename(&tmp, abs)
                .map_err(|e| node_fs_error_message(&e, "rename", &tmp, Some(abs)))
        });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}
