//! Tests of the `edit_note` tool: its modes, the expected-content guard and
//! the edit buffer per vault. The value-parsing cases are unit tests in the
//! tool.

use obsidian_brain::store::edges::get_edges_by_target;
use serde_json::{Value, json};

use crate::support::Vault;

fn with_note(rel: &str, content: &str) -> Vault {
    let v = Vault::new();
    v.write(rel, content);
    v.row(rel, "Note");
    v
}

#[tokio::test]
async fn a_dry_run_returns_a_preview_and_leaves_the_file() {
    let original = "# Draft\n\nSome content.\n";
    let v = with_note("draft.md", original);
    let out = v
        .ok(
            "edit_note",
            json!({ "name": "draft.md", "mode": "append", "content": "X", "dryRun": true }),
        )
        .await;
    assert_eq!(out["dryRun"], true);
    assert!(out["previewId"].as_str().unwrap().starts_with("prev_"));
    assert_eq!(out["path"], "draft.md");
    assert_eq!(out["mode"], "append");
    assert_eq!(
        out["diff"],
        "Index: draft.md\n===================================================================\n--- draft.md\toriginal\n+++ draft.md\tproposed\n@@ -1,3 +1,4 @@\n # Draft\n \n Some content.\n+X\n\\ No newline at end of file\n"
    );
    assert_eq!(v.read("draft.md"), original);
}

#[tokio::test]
async fn bulk_edits_apply_together() {
    let v = with_note("multi.md", "# Note\n\nOriginal body.\n");
    let out = v
        .ok(
            "edit_note",
            json!({
                "name": "multi.md",
                "edits": [
                    { "mode": "append", "content": "\nAppended line.\n" },
                    { "mode": "replace_window", "search": "Original body.", "content": "Updated body." },
                ],
            }),
        )
        .await;
    assert_eq!(out["mode"], "bulk");
    assert_eq!(out["editsApplied"], 2);
    assert!(out["bytesWritten"].as_u64().unwrap() > 0);
    assert_eq!(
        out["path"],
        v.path().join("multi.md").to_string_lossy().as_ref()
    );
    let disk = v.read("multi.md");
    assert!(disk.contains("Updated body."));
    assert!(disk.contains("Appended line."));
    assert!(!disk.contains("Original body."));
}

#[tokio::test]
async fn a_failing_bulk_edit_names_its_index_and_writes_nothing() {
    let original = "# Stable\n\nContent here.\n";
    let v = with_note("stable.md", original);
    let err = v
        .err(
            "edit_note",
            json!({
                "name": "stable.md",
                "edits": [
                    { "mode": "append", "content": "\nExtra.\n" },
                    { "mode": "replace_window", "search": "DOES_NOT_EXIST", "content": "nope" },
                ],
            }),
        )
        .await;
    assert!(err.contains("edits[1]"), "{err}");
    assert_eq!(v.read("stable.md"), original);
}

#[tokio::test]
async fn a_bulk_dry_run_returns_a_preview_without_writing() {
    let original = "# Preview\n\nOriginal.\n";
    let v = with_note("preview.md", original);
    let out = v
        .ok(
            "edit_note",
            json!({
                "name": "preview.md",
                "dryRun": true,
                "edits": [{ "mode": "append", "content": "\nNew section.\n" }],
            }),
        )
        .await;
    assert_eq!(out["dryRun"], true);
    assert!(out["previewId"].as_str().unwrap().starts_with("prev_"));
    assert_eq!(out["mode"], "bulk");
    assert_eq!(out["editsApplied"], 1);
    assert!(out["diff"].as_str().unwrap().contains("+New section."));
    assert_eq!(v.read("preview.md"), original);
}

#[tokio::test]
async fn a_bulk_item_missing_a_field_is_named() {
    let v = with_note("n.md", "x\n");
    let err = v
        .err(
            "edit_note",
            json!({ "name": "n.md", "edits": [{ "mode": "append" }] }),
        )
        .await;
    assert_eq!(err, "edits[0]: edit_note mode=append requires 'content'");
}

#[tokio::test]
async fn a_mode_is_required_without_edits() {
    let v = with_note("n.md", "x\n");
    let err = v.err("edit_note", json!({ "name": "n.md" })).await;
    assert_eq!(
        err,
        "edit_note: 'mode' is required when 'edits' is not provided"
    );
}

#[tokio::test]
async fn patch_frontmatter_reads_value_json_and_edits_in_place() {
    let v = with_note("fm.md", "---\n# keep\ntitle: \"T\"\ncount: 1\n---\nbody\n");
    v.ok(
        "edit_note",
        json!({ "name": "fm.md", "mode": "patch_frontmatter", "key": "count", "valueJson": "42" }),
    )
    .await;
    assert_eq!(
        v.read("fm.md"),
        "---\n# keep\ntitle: \"T\"\ncount: 42\n---\nbody\n"
    );
    // `valueJson` wins over `value`.
    v.ok(
        "edit_note",
        json!({ "name": "fm.md", "mode": "patch_frontmatter", "key": "done", "value": "no", "valueJson": "true" }),
    )
    .await;
    assert!(v.read("fm.md").contains("done: true\n"));
    // `value: null` clears the key.
    v.ok(
        "edit_note",
        json!({ "name": "fm.md", "mode": "patch_frontmatter", "key": "done", "value": null }),
    )
    .await;
    assert!(!v.read("fm.md").contains("done"));
    let err = v
        .err(
            "edit_note",
            json!({ "name": "fm.md", "mode": "patch_frontmatter", "key": "x", "valueJson": "{not json" }),
        )
        .await;
    assert!(err.contains("valueJson is not valid JSON"), "{err}");
}

#[tokio::test]
async fn patch_heading_reports_removed_len() {
    let v = with_note("h.md", "# A\n\nold\n\n# B\n\nkeep\n");
    let out = v
        .ok(
            "edit_note",
            json!({ "name": "h.md", "mode": "patch_heading", "heading": "A", "content": "\nnew\n\n" }),
        )
        .await;
    assert_eq!(out["mode"], "patch_heading");
    assert!(out["removedLen"].as_u64().unwrap() > 0);
    assert!(out["diff"]["before"].is_string());
    assert!(v.read("h.md").contains("new"));
    assert!(v.read("h.md").contains("keep"));
}

mod expected_content {
    use super::*;

    const NOTE: &str = "note.md";
    const ORIGINAL: &str = "# Plan\n\nStep one.\nStep two.\n\n# Notes\n\nKeep this.\n";

    fn vault() -> Vault {
        with_note(NOTE, ORIGINAL)
    }

    async fn edit(v: &Vault, mut args: Value) -> Result<Value, String> {
        args["name"] = NOTE.into();
        v.call("edit_note", args).await
    }

    #[tokio::test]
    async fn a_replace_window_edit_whose_text_still_matches_applies() {
        let v = vault();
        edit(
            &v,
            json!({ "mode": "replace_window", "search": "Step two.", "content": "Step 2.", "expectedContent": "Step two." }),
        )
        .await
        .unwrap();
        assert!(v.read(NOTE).contains("Step 2."));
    }

    #[tokio::test]
    async fn a_patch_heading_replace_applies_when_the_section_matches() {
        let v = vault();
        edit(
            &v,
            json!({
                "mode": "patch_heading", "heading": "Plan", "headingOp": "replace",
                "content": "\nOnly step.\n\n", "expectedContent": "\nStep one.\nStep two.\n\n",
            }),
        )
        .await
        .unwrap();
        assert!(v.read(NOTE).contains("Only step."));
        assert!(v.read(NOTE).contains("Keep this."));
    }

    #[tokio::test]
    async fn a_patch_heading_replace_is_refused_when_the_section_changed() {
        let v = vault();
        let err = edit(
            &v,
            json!({
                "mode": "patch_heading", "heading": "Plan", "headingOp": "replace",
                "content": "\nOnly step.\n\n", "expectedContent": "Step one.\nStep three.",
            }),
        )
        .await
        .unwrap_err();
        assert!(err.contains("no longer matches expectedContent"), "{err}");
        assert!(err.contains("Step two."), "{err}");
        assert_eq!(v.read(NOTE), ORIGINAL);
    }

    #[tokio::test]
    async fn an_at_line_replace_applies_when_the_line_matches() {
        let v = vault();
        edit(
            &v,
            json!({ "mode": "at_line", "line": 3, "lineOp": "replace", "content": "First step.", "expectedContent": "Step one." }),
        )
        .await
        .unwrap();
        assert_eq!(v.read(NOTE).split('\n').nth(2), Some("First step."));
    }

    #[tokio::test]
    async fn an_at_line_replace_on_a_moved_line_is_refused() {
        let v = vault();
        let err = edit(
            &v,
            json!({ "mode": "at_line", "line": 4, "lineOp": "replace", "content": "First step.", "expectedContent": "Step one." }),
        )
        .await
        .unwrap_err();
        assert!(err.contains("no longer matches"), "{err}");
        assert_eq!(v.read(NOTE), ORIGINAL);
    }

    #[tokio::test]
    async fn line_endings_and_trailing_whitespace_do_not_count() {
        let v = vault();
        edit(
            &v,
            json!({
                "mode": "patch_heading", "heading": "Plan", "headingOp": "replace",
                "content": "\nOnly step.\n\n", "expectedContent": "Step one.  \r\nStep two.\r\n",
            }),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_dry_run_with_stale_text_makes_no_preview() {
        let v = vault();
        let err = edit(
            &v,
            json!({ "mode": "at_line", "line": 3, "lineOp": "replace", "content": "X", "expectedContent": "something else", "dryRun": true }),
        )
        .await
        .unwrap_err();
        assert!(err.contains("no longer matches"), "{err}");
    }

    #[tokio::test]
    async fn a_dry_run_with_matching_text_passes() {
        let v = vault();
        let out = edit(
            &v,
            json!({ "mode": "at_line", "line": 3, "lineOp": "replace", "content": "X", "expectedContent": "Step one.", "dryRun": true }),
        )
        .await
        .unwrap();
        assert_eq!(out["dryRun"], true);
        assert_eq!(v.read(NOTE), ORIGINAL);
    }

    #[tokio::test]
    async fn an_edit_that_replaces_nothing_refuses_expected_content() {
        let v = vault();
        let err = edit(
            &v,
            json!({ "mode": "append", "content": "\nMore.", "expectedContent": "anything" }),
        )
        .await
        .unwrap_err();
        assert!(
            err.contains("applies only to edits that replace text"),
            "{err}"
        );
        assert_eq!(v.read(NOTE), ORIGINAL);
    }

    #[tokio::test]
    async fn bulk_edits_refuse_expected_content() {
        let v = vault();
        let err = edit(
            &v,
            json!({ "edits": [{ "mode": "append", "content": "x" }], "expectedContent": "Step one." }),
        )
        .await
        .unwrap_err();
        assert!(err.contains("single edit, not with `edits`"), "{err}");
    }
}

mod from_buffer {
    use super::*;

    const NOTE: &str = "shared.md";
    const ORIGINAL: &str = "# Shared\n\nThe quick brown fox jumps over the lazy dog.\n";

    /// A near miss: the exact match fails, the fuzzy retry succeeds.
    fn failing_edit() -> Value {
        json!({
            "name": NOTE,
            "mode": "replace_window",
            "search": "The quick brown fax jumps over the lazy dog.",
            "content": "The slow red fox naps.",
        })
    }

    #[tokio::test]
    async fn a_failed_edit_is_retried_from_the_buffer_in_the_same_vault() {
        let alpha = with_note(NOTE, ORIGINAL);
        alpha.err("edit_note", failing_edit()).await;
        let out = alpha
            .ok("edit_note", json!({ "name": NOTE, "from_buffer": true }))
            .await;
        assert_eq!(out["mode"], "replace_window");
        assert!(alpha.read(NOTE).contains("The slow red fox naps."));
        // Cleared on success.
        let err = alpha
            .err("edit_note", json!({ "name": NOTE, "from_buffer": true }))
            .await;
        assert!(err.contains("No buffered edit found"), "{err}");
    }

    #[tokio::test]
    async fn another_vault_never_sees_the_failed_edit() {
        let alpha = with_note(NOTE, ORIGINAL);
        let beta = with_note(NOTE, ORIGINAL);
        alpha.err("edit_note", failing_edit()).await;
        let err = beta
            .err("edit_note", json!({ "name": NOTE, "from_buffer": true }))
            .await;
        assert!(err.contains("No buffered edit found"), "{err}");
        assert_eq!(beta.read(NOTE), ORIGINAL);
    }

    #[tokio::test]
    async fn bulk_edits_refuse_from_buffer() {
        let v = with_note(NOTE, ORIGINAL);
        let err = v
            .err(
                "edit_note",
                json!({ "name": NOTE, "from_buffer": true, "edits": [{ "mode": "append", "content": "x" }] }),
            )
            .await;
        assert!(err.contains("from_buffer is not compatible"), "{err}");
    }
}

#[tokio::test]
async fn an_appended_link_becomes_an_edge_after_the_reindex() {
    let v = Vault::new();
    v.write("Cars.md", "# Cars\n\nNo links yet.\n");
    v.write("BMW.md", "# BMW\n\nThe car.\n");
    v.index().await;
    assert!(
        v.db()
            .with(|c| get_edges_by_target(c, "BMW.md"))
            .unwrap()
            .is_empty()
    );
    v.ok(
        "edit_note",
        json!({ "name": "Cars", "mode": "append", "content": "See [[BMW]] for details." }),
    )
    .await;
    v.ctx.wait_for_background_reindex().await;
    let inbound = v.db().with(|c| get_edges_by_target(c, "BMW.md")).unwrap();
    assert!(inbound.iter().any(|e| e.source_id == "Cars.md"));
}

#[tokio::test]
async fn out_of_range_arguments_are_refused() {
    let v = with_note("n.md", "x\n");
    let err = v
        .err(
            "edit_note",
            json!({ "name": "n.md", "mode": "at_line", "line": 0, "content": "x" }),
        )
        .await;
    assert!(err.contains("line"), "{err}");
    let err = v
        .err(
            "edit_note",
            json!({ "name": "n.md", "mode": "replace_window", "search": "x", "content": "y", "fuzzyThreshold": 2 }),
        )
        .await;
    assert!(err.contains("fuzzyThreshold"), "{err}");
    assert_eq!(v.read("n.md"), "x\n");
}
