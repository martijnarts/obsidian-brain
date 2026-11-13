//! `update_properties`: set and remove several frontmatter keys of one note
//! in a single write, editing only the lines of the keys that change.

use anyhow::bail;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::mcp::tool::ToolSpec;
use crate::resolve::single_note::resolve_single_note;
use crate::tools::support::background_reindex::run_background_reindex;
use crate::util::errors::node_fs_error_message;
use crate::util::js;
use crate::vault::editor::update_frontmatter;
use crate::vault::vault_path::{read_note_file, write_file_atomic};

const DESCRIPTION: &str = "Set and remove several frontmatter properties of one note in a single write. Creates the frontmatter block if the note has none; the body and keys not named stay as they are. Pass `dryRun: true` to see the resulting frontmatter without writing.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Path or fuzzy match of the note.
    name: String,
    /// Keys to write, with their values. A `null` value removes the key.
    set: Option<Map<String, Value>>,
    /// Keys to remove. Absent keys are skipped.
    remove: Option<Vec<String>>,
    /// Return the new frontmatter without writing.
    dry_run: Option<bool>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("update_properties", DESCRIPTION, |ctx, args| async move {
        let set = args.set.unwrap_or_default();
        let remove = args.remove.unwrap_or_default();
        if set.is_empty() && remove.is_empty() {
            bail!("Nothing to do: pass at least one key in `set` or `remove`.");
        }
        // `Object.keys(set)` lists index-like keys first.
        let mut ordered = set.clone();
        js::js_object_order(&mut ordered);
        let invalid: Vec<String> = ordered
            .keys()
            .chain(&remove)
            .filter(|k| js::trim(k).is_empty() || k.contains(['\r', '\n']))
            .map(|k| json!(k).to_string())
            .collect();
        if !invalid.is_empty() {
            bail!("Invalid frontmatter key(s): {}", invalid.join(", "));
        }

        let file_rel_path = ctx
            .db
            .with(|conn| resolve_single_note(&args.name, conn, false))?;
        let note = read_note_file(&ctx.config.vault_path, &file_rel_path)?;
        let res = update_frontmatter(&note.content, &set, &remove)?;
        let mut summary = Map::new();
        if args.dry_run == Some(true) {
            summary.insert("dryRun".into(), true.into());
        }
        summary.insert("path".into(), file_rel_path.into());
        summary.insert("set".into(), json!(res.set));
        summary.insert("removed".into(), json!(res.removed));
        if !res.not_present.is_empty() {
            summary.insert("notPresent".into(), json!(res.not_present));
        }
        summary.insert("frontmatter".into(), Value::Object(res.frontmatter));
        if args.dry_run == Some(true) {
            return Ok(Value::Object(summary));
        }

        if res.next != note.content {
            write_file_atomic(&note.abs, &res.next)
                .map_err(|e| anyhow::anyhow!(node_fs_error_message(&e, "open", &note.abs, None)))?;
            run_background_reindex(&ctx);
        }
        Ok(Value::Object(summary))
    })
}
