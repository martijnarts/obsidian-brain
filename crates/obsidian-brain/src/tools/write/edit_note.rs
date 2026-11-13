//! `edit_note`: in-place edits of an existing note. The six modes map onto
//! [`EditMode`]; a mode without the fields it needs fails loudly instead of
//! reaching the editor half-filled.

use anyhow::{Result, anyhow, bail};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::mcp::tool::ToolSpec;
use crate::resolve::resolve_single_note;
use crate::tools::support::patch::create_patch;
use crate::tools::support::{
    BufferEntry, EDIT_BUFFER, PREVIEW_STORE, PendingEdit, run_background_reindex,
};
use crate::util::errors::node_fs_error_message;
use crate::util::js::now_ms;
use crate::vault::editor::{
    self, EditMode, HeadingOp, HeadingScope, LineOp, apply_edit, bulk_edit_note,
    check_expected_content,
};
use crate::vault::vault_path::{read_utf8, resolve_vault_path};

const DESCRIPTION: &str = "Modify an existing note. Supports six edit modes: append (add to end; defensively inserts a leading newline if the source didn't end with one), prepend (insert after frontmatter if present, otherwise at file start), replace_window (find a block of text and replace it — optionally fuzzy; fuzzy extends match to consume trailing .?! so the replacement has no doubled punctuation), patch_heading (insert or replace content under a specific heading; `headingOp: 'before' | 'after'` inserts immediately before/after the heading line — use `before` on the NEXT heading to append to a section's end; `headingOp: 'replace'` with `scope: 'section'` (default) replaces to the next same-or-higher heading or EOF — CAREFUL on the LAST heading, this consumes everything below including content separated by blank lines — pass `scope: 'body'` to stop at the first blank line after the body; if the target heading text appears MORE THAN ONCE the call throws MultipleMatches listing each occurrence with its line number — pass `headingIndex: 0 | 1 | ...` (0-indexed, top-to-bottom) to pick one), patch_frontmatter (set a single YAML key; pass `value: null` to clear — or from XML-stringifying clients use `valueJson: 'null'` for true null, `valueJson: 'true'` for a real boolean, `valueJson: '42'` for a number, `valueJson: '[\"a\"]'` for an array; `valueJson` wins over `value` when both are set), at_line (insert or replace at a 1-indexed line number that counts from file start including frontmatter lines). Pass `edits` (array) to apply multiple edits atomically — all succeed or none are written.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Append,
    Prepend,
    ReplaceWindow,
    PatchHeading,
    PatchFrontmatter,
    AtLine,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Append => "append",
            Mode::Prepend => "prepend",
            Mode::ReplaceWindow => "replace_window",
            Mode::PatchHeading => "patch_heading",
            Mode::PatchFrontmatter => "patch_frontmatter",
            Mode::AtLine => "at_line",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum HeadingOpArg {
    Replace,
    Before,
    After,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum ScopeArg {
    Section,
    Body,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum LineOpArg {
    Before,
    After,
    Replace,
}

// One item of `edits`. The TS schema gave these fields no descriptions.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct EditItem {
    mode: Mode,
    content: Option<String>,
    search: Option<String>,
    fuzzy: Option<bool>,
    #[schemars(range(min = 0, max = 1))]
    fuzzy_threshold: Option<f64>,
    heading: Option<String>,
    heading_op: Option<HeadingOpArg>,
    scope: Option<ScopeArg>,
    heading_index: Option<u64>,
    key: Option<String>,
    #[serde(default)]
    #[schemars(schema_with = "any_value")]
    value: Option<Value>,
    value_json: Option<String>,
    #[schemars(range(min = 1))]
    line: Option<u64>,
    line_op: Option<LineOpArg>,
}

/// zod's `z.unknown()`: the empty schema.
fn any_value(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({})
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Path or fuzzy match of the note to edit.
    name: String,
    /// Edit mode. Required unless `edits` array is provided.
    mode: Option<Mode>,
    /// New content to insert or replace (meaning is mode-dependent).
    content: Option<String>,
    /// For `replace_window`: the block of text to locate and replace.
    search: Option<String>,
    /// For `replace_window`: tolerate whitespace and trailing-punctuation drift.
    fuzzy: Option<bool>,
    /// Similarity threshold for fuzzy replace_window matches (0-1, default 0.7). Higher = stricter. 0.9 is recommended for typo-tolerant matching of known-good text.
    #[schemars(range(min = 0, max = 1))]
    fuzzy_threshold: Option<f64>,
    /// Target heading text for `patch_heading` mode.
    heading: Option<String>,
    /// For `patch_heading`. `replace` (default) replaces section; `before`/`after` inserts adjacent to the heading line.
    heading_op: Option<HeadingOpArg>,
    /// For `patch_heading replace`: `section` (default) consumes to next same-or-higher heading; `body` stops at first blank line.
    scope: Option<ScopeArg>,
    /// For `patch_heading` when the heading appears more than once — 0-indexed top-to-bottom picker.
    heading_index: Option<u64>,
    /// For `patch_frontmatter`: the YAML key to set.
    key: Option<String>,
    /// For `patch_frontmatter`: value to set. Use `null` to clear a key. Prefer `valueJson` from clients that stringify params.
    value: Option<Value>,
    /// For `patch_frontmatter`: JSON-encoded value (wins over `value`). Use `"null"` to clear, `"true"` for boolean, `"42"` for number.
    value_json: Option<String>,
    /// For `at_line`: 1-indexed line number (counts from file start including frontmatter).
    #[schemars(range(min = 1))]
    line: Option<u64>,
    /// For `at_line`: insert before/after the target line, or replace it. Default `replace`.
    line_op: Option<LineOpArg>,
    /// Optional guard for edits that replace text: the exact text being replaced, as last read. If the note changed since, the edit fails and nothing is written. Not with `edits`.
    expected_content: Option<String>,
    /// If true, return a unified-diff preview without writing. Pass the returned previewId to apply_edit_preview to commit.
    dry_run: Option<bool>,
    /// If true, retry a previously failed replace_window edit using the cached content + search with fuzzy: true, fuzzyThreshold: 0.5. Use when the prior edit failed with NoMatch. Cleared on success.
    #[serde(rename = "from_buffer")]
    from_buffer: Option<bool>,
    /// Array of edits to apply atomically to a single file. If any edit fails, no edit is written. Applied in order against the accumulated state (each edit sees the result of previous edits in the batch). Use replace_window / patch_heading for content-anchored edits; at_line references the accumulated state, not the original file.
    edits: Option<Vec<EditItem>>,
}

impl Args {
    /// The single-edit fields, shaped like one item of `edits`.
    fn item(&self, mode: Mode) -> EditItem {
        EditItem {
            mode,
            content: self.content.clone(),
            search: self.search.clone(),
            fuzzy: self.fuzzy,
            fuzzy_threshold: self.fuzzy_threshold,
            heading: self.heading.clone(),
            heading_op: self.heading_op,
            scope: self.scope,
            heading_index: self.heading_index,
            key: self.key.clone(),
            value: self.value.clone(),
            value_json: self.value_json.clone(),
            line: self.line,
            line_op: self.line_op,
        }
    }
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("edit_note", DESCRIPTION, |ctx, args| async move {
        let name = args.name.clone();
        let note_path = ctx
            .db
            .run(move |conn| resolve_single_note(&name, conn, false))
            .await?;
        // The id comes from the index; guard it all the same.
        let abs = resolve_vault_path(&ctx.config.vault_path, &note_path)?.abs;
        let vault_key = ctx.config.vault_path.to_string_lossy().into_owned();
        let buffer_key = abs.to_string_lossy().into_owned();

        if let Some(edits) = args.edits.as_ref().filter(|e| !e.is_empty()) {
            if args.expected_content.is_some() {
                bail!("expectedContent works with a single edit, not with `edits`.");
            }
            if args.from_buffer == Some(true) {
                bail!(
                    "from_buffer is not compatible with bulk edits; re-issue the buffer retry as a single edit."
                );
            }
            let modes = edits
                .iter()
                .enumerate()
                .map(|(i, item)| build_edit_mode(item).map_err(|e| anyhow!("edits[{i}]: {e}")))
                .collect::<Result<Vec<_>>>()?;

            if args.dry_run == Some(true) {
                let original = read_for_dry_run(&abs, &note_path)?;
                let mut proposed = original.clone();
                for (i, mode) in modes.iter().enumerate() {
                    proposed = apply_edit(&proposed, mode)
                        .map_err(|e| {
                            anyhow!(
                                "[bulk edit dryRun] edits[{i}] ({}) failed: {e}. No edits were applied.",
                                mode.kind()
                            )
                        })?
                        .next;
                }
                let diff = create_patch(&note_path, &original, &proposed, "original", "proposed");
                let preview_id = new_preview_id();
                PREVIEW_STORE.set(PendingEdit {
                    preview_id: preview_id.clone(),
                    vault_path: vault_key,
                    path: note_path.clone(),
                    original_content: original,
                    proposed_content: proposed,
                    diff: diff.clone(),
                    mode: "bulk".into(),
                    created_at: now_ms(),
                });
                return Ok(json!({
                    "dryRun": true,
                    "previewId": preview_id,
                    "path": note_path,
                    "diff": diff,
                    "mode": "bulk",
                    "editsApplied": modes.len(),
                }));
            }

            let result = bulk_edit_note(&ctx.config.vault_path, &note_path, &modes)?;
            run_background_reindex(&ctx);
            return Ok(json!({
                "path": result.path.to_string_lossy(),
                "mode": "bulk",
                "editsApplied": result.edits_applied,
                "bytesWritten": result.bytes_written,
            }));
        }

        // A buffer retry never buffers its own failure, which would loop.
        let is_buffer_retry = args.from_buffer == Some(true);
        let edit_mode = if is_buffer_retry {
            let Some(buffered) = EDIT_BUFFER.get(&buffer_key) else {
                bail!(
                    "No buffered edit found for \"{note_path}\". Buffer TTL is 30 minutes. Re-issue the edit with explicit content."
                );
            };
            EditMode::ReplaceWindow {
                search: buffered.search,
                content: buffered.content,
                fuzzy: true,
                fuzzy_threshold: Some(0.5),
            }
        } else {
            let Some(mode) = args.mode else {
                bail!("edit_note: 'mode' is required when 'edits' is not provided");
            };
            build_edit_mode(&args.item(mode))?
        };
        // The mode as the caller named it; a buffer retry may name none.
        let mode_name = args.mode.map(Mode::as_str).unwrap_or("");

        if args.dry_run == Some(true) {
            let original = read_for_dry_run(&abs, &note_path)?;
            let applied = apply_edit(&original, &edit_mode)?;
            if let Some(expected) = &args.expected_content {
                check_expected_content(&original, &applied, expected)?;
            }
            let diff = create_patch(&note_path, &original, &applied.next, "original", "proposed");
            let preview_id = new_preview_id();
            PREVIEW_STORE.set(PendingEdit {
                preview_id: preview_id.clone(),
                vault_path: vault_key,
                path: note_path.clone(),
                original_content: original,
                proposed_content: applied.next,
                diff: diff.clone(),
                mode: mode_name.into(),
                created_at: now_ms(),
            });
            let mut out = Map::new();
            out.insert("dryRun".into(), true.into());
            out.insert("previewId".into(), preview_id.into());
            out.insert("path".into(), note_path.into());
            out.insert("diff".into(), diff.into());
            if !mode_name.is_empty() {
                out.insert("mode".into(), mode_name.into());
            }
            return Ok(Value::Object(out));
        }

        let result = match editor::edit_note(
            &ctx.config.vault_path,
            &note_path,
            &edit_mode,
            args.expected_content.as_deref(),
        ) {
            Ok(r) => r,
            Err(err) => {
                // Keep a failed replace_window edit, so the agent can retry
                // with `from_buffer: true` without sending the content again.
                if !is_buffer_retry
                    && args.mode == Some(Mode::ReplaceWindow)
                    && let (Some(content), Some(search)) = (&args.content, &args.search)
                {
                    EDIT_BUFFER.push(BufferEntry {
                        path: buffer_key,
                        content: content.clone(),
                        search: search.clone(),
                        mode: "replace_window".into(),
                        failed_at: now_ms(),
                        error: format!("{err:#}"),
                    });
                }
                return Err(err);
            }
        };

        // A successful edit, buffer retry or not, clears the buffer.
        EDIT_BUFFER.remove(&buffer_key);

        let mut out = Map::new();
        out.insert("path".into(), result.path.to_string_lossy().into());
        out.insert(
            "mode".into(),
            if is_buffer_retry {
                "replace_window"
            } else {
                mode_name
            }
            .into(),
        );
        out.insert("diff".into(), serde_json::to_value(&result.diff)?);
        out.insert("bytesWritten".into(), result.bytes_written.into());
        // `removedLen` lets a caller see when the default `section` scope
        // consumed more than it expected.
        if args.mode == Some(Mode::PatchHeading) {
            out.insert("removedLen".into(), result.removed_len.into());
        }

        run_background_reindex(&ctx);
        Ok(Value::Object(out))
    })
}

fn read_for_dry_run(abs: &std::path::Path, note_path: &str) -> Result<String> {
    read_utf8(abs).map_err(|e| {
        anyhow!(
            "edit_note dryRun: could not read \"{note_path}\": {}",
            node_fs_error_message(&e, "open", abs, None)
        )
    })
}

/// `prev_` and a random v4 UUID, as `randomUUID()` formats it.
fn new_preview_id() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("the OS random source works");
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!(
        "prev_{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// `JSON.parse(valueJson)` for `patch_frontmatter`.
pub(super) fn parse_value_json(value_json: &str) -> Result<Value> {
    serde_json::from_str(value_json)
        .map_err(|e| anyhow!("edit_note mode=patch_frontmatter: valueJson is not valid JSON: {e}"))
}

fn need<T: Clone>(v: &Option<T>, mode: &str, field: &str) -> Result<T> {
    v.clone()
        .ok_or_else(|| anyhow!("edit_note mode={mode} requires '{field}'"))
}

fn build_edit_mode(a: &EditItem) -> Result<EditMode> {
    let m = a.mode.as_str();
    Ok(match a.mode {
        Mode::Append => EditMode::Append {
            content: need(&a.content, m, "content")?,
        },
        Mode::Prepend => EditMode::Prepend {
            content: need(&a.content, m, "content")?,
        },
        Mode::ReplaceWindow => EditMode::ReplaceWindow {
            search: need(&a.search, m, "search")?,
            content: need(&a.content, m, "content")?,
            fuzzy: a.fuzzy.unwrap_or(false),
            fuzzy_threshold: a.fuzzy_threshold,
        },
        Mode::PatchHeading => EditMode::PatchHeading {
            heading: need(&a.heading, m, "heading")?,
            content: need(&a.content, m, "content")?,
            op: match a.heading_op {
                None | Some(HeadingOpArg::Replace) => HeadingOp::Replace,
                Some(HeadingOpArg::Before) => HeadingOp::Before,
                Some(HeadingOpArg::After) => HeadingOp::After,
            },
            scope: match a.scope {
                None | Some(ScopeArg::Section) => HeadingScope::Section,
                Some(ScopeArg::Body) => HeadingScope::Body,
            },
            heading_index: a.heading_index.map(|i| i as i64),
        },
        Mode::PatchFrontmatter => {
            // `valueJson` wins: it is the path for clients that send every
            // argument as a string.
            let value = match &a.value_json {
                Some(json) => Some(parse_value_json(json)?),
                None => a.value.clone(),
            };
            EditMode::PatchFrontmatter {
                key: need(&a.key, m, "key")?,
                value,
            }
        }
        Mode::AtLine => EditMode::AtLine {
            line: need(&a.line, m, "line")? as i64,
            content: need(&a.content, m, "content")?,
            op: match a.line_op {
                None | Some(LineOpArg::After) => LineOp::After,
                Some(LineOpArg::Before) => LineOp::Before,
                Some(LineOpArg::Replace) => LineOp::Replace,
            },
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_value_json_reads_every_json_type() {
        assert_eq!(parse_value_json("null").unwrap(), Value::Null);
        assert_eq!(parse_value_json("true").unwrap(), json!(true));
        assert_eq!(parse_value_json("42").unwrap(), json!(42));
        assert_eq!(parse_value_json(r#"["a","b"]"#).unwrap(), json!(["a", "b"]));
        assert_eq!(parse_value_json(r#"{"k":1}"#).unwrap(), json!({ "k": 1 }));
        assert_eq!(parse_value_json(r#""hello""#).unwrap(), json!("hello"));
        let err = parse_value_json("{not json").unwrap_err().to_string();
        assert!(err.contains("valueJson is not valid JSON"), "{err}");
    }

    #[test]
    fn preview_ids_look_like_prefixed_v4_uuids() {
        let id = new_preview_id();
        let re = regex::Regex::new(
            r"^prev_[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$",
        )
        .unwrap();
        assert!(re.is_match(&id), "{id}");
    }
}
