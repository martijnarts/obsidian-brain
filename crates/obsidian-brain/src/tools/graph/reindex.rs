//! `reindex`: an incremental index of the vault, then a prune of orphan
//! stubs. The MCP layer gives it a 10-minute timeout.

use serde_json::Value;

use crate::mcp::tool::{NoArgs, ToolSpec};
use crate::store::nodes::prune_all_orphan_stubs;

const DESCRIPTION: &str =
    "Re-index the vault: re-embeds notes whose mtime changed and prunes orphan stubs.";

/// What `index_status` reports as the last reindex reason after this tool.
pub const MANUAL_REINDEX_REASON: &str = "user-triggered reindex";

pub fn spec() -> ToolSpec {
    ToolSpec::new::<NoArgs, _, _>("reindex", DESCRIPTION, |ctx, _args| async move {
        // An explicit reindex waits for the embedder; only `search` answers
        // "preparing" while it loads.
        ctx.ensure_embedder_ready().await?;
        ctx.set_last_manual_reindex_reason(Some(MANUAL_REINDEX_REASON.into()));
        let stats = ctx.pipeline.index(&ctx.config.vault_path).await?;
        let stubs_pruned = ctx.db.run(prune_all_orphan_stubs).await?;
        let mut out = match serde_json::to_value(stats)? {
            Value::Object(map) => map,
            _ => unreachable!("IndexStats serializes to an object"),
        };
        out.insert("stubsPruned".into(), stubs_pruned.into());
        Ok(Value::Object(out))
    })
}
