//! `link_notes`: append a wiki-link with a context sentence to a source
//! note. The target may be a note that does not exist yet; only the source
//! must be in the index.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::require_non_empty;
use crate::mcp::tool::ToolSpec;
use crate::resolve::resolve_single_note;
use crate::tools::support::run_background_reindex;

const DESCRIPTION: &str = "Add a wiki-link from one note to another with a context sentence describing why they're connected. Appends the link to the source note and records the edge in the graph so analytics pick it up.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Source note to add the link from (path or fuzzy match).
    source: String,
    /// Target note to link to (path, title, or new wiki-link ref).
    target: String,
    /// One-sentence explanation of why these notes are connected.
    #[schemars(length(min = 1))]
    context: String,
    /// If true, return the line that would be appended without writing.
    dry_run: Option<bool>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("link_notes", DESCRIPTION, |ctx, args| async move {
        require_non_empty("context", &args.context)?;
        let Args {
            source,
            target,
            context,
            dry_run,
        } = args;

        let job = ctx.clone();
        let (t, c) = (target.clone(), context.clone());
        let source_id = ctx
            .db
            .run(move |conn| -> anyhow::Result<String> {
                let source_id = resolve_single_note(&source, conn, false)?;
                if dry_run == Some(true) {
                    return Ok(source_id);
                }
                job.writer.add_link(conn, &source_id, &t, &c)?;
                Ok(source_id)
            })
            .await?;

        if dry_run == Some(true) {
            // The line `VaultWriter::add_link` would append.
            let would_append = format!("\n{context} [[{target}]]");
            return Ok(json!({
                "dryRun": true,
                "source": source_id,
                "target": target,
                "context": context,
                "wouldAppend": would_append,
            }));
        }

        run_background_reindex(&ctx);
        Ok(json!({ "source": source_id, "target": target, "context": context }))
    })
}
