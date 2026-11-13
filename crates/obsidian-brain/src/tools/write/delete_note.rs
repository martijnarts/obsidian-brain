//! `delete_note`: remove a note from disk and its rows from the index.
//! `confirm` must be the literal `true`, so a call with the argument
//! missing cannot delete anything.
//!
//! When the delete removed edges, the result comes in the `next_actions`
//! envelope with a hint to look for notes it left orphaned; `data` holds
//! the plain result.

use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::invalid;
use crate::mcp::tool::ToolSpec;
use crate::resolve::resolve_single_note;
use crate::store::edges::{count_edges_by_source, count_edges_by_target, get_edges_by_source};
use crate::store::nodes::get_node;
use crate::tools::support::{ContextualResult, NextAction, ToolContext, run_background_reindex};
use crate::vault::mover::delete_note;

const DESCRIPTION: &str = "Permanently delete a note. Removes the file from disk AND its index rows (edges, embedding, node). Requires `confirm: true` to guard against accidents. When the delete removes inbound edges, the response is wrapped in a next_actions envelope suggesting a follow-up `rank_notes(method=pagerank, minIncomingLinks=0)` to spot newly orphaned notes.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Path or fuzzy match of the note to delete.
    name: String,
    /// Must literally be `true` to execute. Guards against accidental deletion.
    #[schemars(extend("const" = true))]
    confirm: bool,
    /// If true, report what would be deleted without removing any files.
    dry_run: Option<bool>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("delete_note", DESCRIPTION, |ctx, args| async move {
        if !args.confirm {
            return Err(invalid("`confirm` must be true"));
        }
        let dry_run = args.dry_run == Some(true);
        let job = ctx.clone();
        let out = ctx
            .db
            .run(move |conn| -> Result<Value> {
                let file_rel_path = resolve_single_note(&args.name, conn, true)?;

                if dry_run {
                    let node = get_node(conn, &file_rel_path)?;
                    let edges = count_edges_by_source(conn, &file_rel_path)?;
                    // Stubs this note is the only one to link to.
                    let mut stubs_to_prune = 0;
                    for e in get_edges_by_source(conn, &file_rel_path)? {
                        if e.target_id.starts_with("_stub/")
                            && count_edges_by_target(conn, &e.target_id)? == 1
                        {
                            stubs_to_prune += 1;
                        }
                    }
                    return Ok(json!({
                        "dryRun": true,
                        "wouldDelete": {
                            "path": file_rel_path,
                            "node": node.is_some(),
                            "edges": edges,
                            "stubsToPrune": stubs_to_prune,
                        },
                    }));
                }

                let result = delete_note(&job.config.vault_path, &file_rel_path, conn)?;
                let edges_removed = result.deleted_from_index.edges;
                if edges_removed == 0 {
                    return Ok(serde_json::to_value(&result)?);
                }
                let mut hint_args = Map::new();
                hint_args.insert("metric".into(), "influence".into());
                hint_args.insert("minIncomingLinks".into(), 0.into());
                let envelope = ContextualResult {
                    data: result,
                    context: ToolContext {
                        state: None,
                        next_actions: Some(vec![NextAction {
                            description: "Check for newly orphaned notes".into(),
                            tool: "rank_notes".into(),
                            args: hint_args,
                            reason: format!(
                                "Removed {edges_removed} edge{} — some notes may now be orphans",
                                if edges_removed == 1 { "" } else { "s" }
                            ),
                        }]),
                    },
                };
                Ok(serde_json::to_value(&envelope)?)
            })
            .await?;

        if !dry_run {
            run_background_reindex(&ctx);
        }
        Ok(out)
    })
}
