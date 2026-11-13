//! `create_note`: write a new `.md` file into the vault and index it, so it
//! shows up in search and graph tools at once.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{basename_md, require_non_empty};
use crate::mcp::tool::ToolSpec;
use crate::store::nodes::migrate_stub_to_real;
use crate::tools::support::run_background_reindex;
use crate::vault::writer::CreateNodeOptions;

const DESCRIPTION: &str = "Create a new note in the vault with a title, body, and optional YAML frontmatter. The new note is indexed immediately so semantic search and graph tools can find it. Auto-injects a `title:` field into frontmatter matching the note title unless frontmatter already has one.";

#[derive(Debug, Deserialize, JsonSchema)]
struct Args {
    /// Note title. Used as the filename base and auto-injected into frontmatter.
    #[schemars(length(min = 1))]
    title: String,
    /// Markdown body (do not include frontmatter here).
    content: String,
    /// Vault-relative subdirectory to create the note in.
    directory: Option<String>,
    /// YAML frontmatter key/value map. `title` is auto-injected unless explicitly set.
    frontmatter: Option<Map<String, Value>>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("create_note", DESCRIPTION, |ctx, args| async move {
        require_non_empty("title", &args.title)?;
        let title = args.title.clone();
        let job = ctx.clone();
        let path = ctx
            .db
            .run(move |conn| -> anyhow::Result<String> {
                let path = job.writer.create_node(
                    conn,
                    &CreateNodeOptions {
                        title: args.title,
                        content: args.content,
                        directory: args.directory,
                        frontmatter: args.frontmatter.unwrap_or_default(),
                    },
                )?;
                // Another note may have linked to this one before it existed,
                // which made a `_stub/<stem>.md` node. Repoint its inbound
                // edges and drop it before the call returns, so the next call
                // sees the right edges.
                let stem = basename_md(&path);
                if !stem.is_empty() {
                    migrate_stub_to_real(conn, &format!("_stub/{stem}.md"), &path)?;
                }
                Ok(path)
            })
            .await?;

        // The write has landed. The reindex runs in the background: waiting
        // for the embedder on first run would time the call out.
        run_background_reindex(&ctx);
        Ok(json!({ "path": path, "title": title }))
    })
}
