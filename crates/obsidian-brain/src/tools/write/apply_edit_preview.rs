//! `apply_edit_preview`: write an `edit_note` dry run, if the note has not
//! changed since.

use anyhow::{anyhow, bail};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::mcp::tool::ToolSpec;
use crate::tools::support::{PREVIEW_STORE, run_background_reindex};
use crate::util::errors::node_fs_error_message;
use crate::vault::vault_path::{read_utf8, resolve_vault_path, write_file_atomic};

const DESCRIPTION: &str = "Apply a previously previewed edit. Pass the `previewId` returned by `edit_note` with `dryRun: true`. Previews expire after 5 minutes. Fails with a descriptive error if the target file changed since the preview was generated — in that case, regenerate the preview and try again.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// The previewId returned by `edit_note` with `dryRun: true`.
    preview_id: String,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("apply_edit_preview", DESCRIPTION, |ctx, args| async move {
        let vault_key = ctx.config.vault_path.to_string_lossy();
        let Some(preview) = PREVIEW_STORE
            .get(&args.preview_id)
            .filter(|p| p.vault_path == vault_key)
        else {
            bail!(
                "Preview \"{}\" not found or expired (TTL: 5 minutes). Re-run the original edit_note call with dryRun: true to generate a fresh preview.",
                args.preview_id
            );
        };

        let abs = resolve_vault_path(&ctx.config.vault_path, &preview.path)?.abs;
        let current =
            read_utf8(&abs).map_err(|e| anyhow!(node_fs_error_message(&e, "open", &abs, None)))?;
        if current != preview.original_content {
            bail!(
                "File \"{}\" has changed since the preview was generated. Re-run edit_note with dryRun: true to get a fresh preview.",
                preview.path
            );
        }

        write_file_atomic(&abs, &preview.proposed_content)
            .map_err(|e| anyhow!(node_fs_error_message(&e, "open", &abs, None)))?;
        PREVIEW_STORE.delete(&args.preview_id);

        run_background_reindex(&ctx);

        let mut out = Map::new();
        out.insert("path".into(), preview.path.into());
        out.insert("bytesWritten".into(), preview.proposed_content.len().into());
        // A buffer-retry preview has no mode; JSON leaves it out.
        if !preview.mode.is_empty() {
            out.insert("mode".into(), preview.mode.into());
        }
        out.insert("diff".into(), preview.diff.into());
        Ok(Value::Object(out))
    })
}
