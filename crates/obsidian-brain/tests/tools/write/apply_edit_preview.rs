//! Ported from `test/tools/apply-edit-preview.test.ts`, the
//! `apply_edit_preview` case of `test/integration/graph-tools.test.ts` and
//! the preview case of `test/integration/multi-vault.test.ts`.

use obsidian_brain::store::edges::get_edges_by_target;
use obsidian_brain::tools::support::PREVIEW_STORE;
use serde_json::json;

use crate::support::Vault;

#[tokio::test]
async fn a_dry_run_then_apply_writes_the_file_and_clears_the_preview() {
    let v = Vault::new();
    v.write("happy.md", "# Hello\n");
    v.node("happy.md", "Happy");
    let preview = v
        .ok(
            "edit_note",
            json!({ "name": "happy.md", "mode": "append", "content": "\nAppended.", "dryRun": true }),
        )
        .await;
    let id = preview["previewId"].as_str().unwrap().to_string();
    assert!(preview["diff"].as_str().unwrap().contains("Appended"));
    assert_eq!(v.read("happy.md"), "# Hello\n");

    let applied = v.ok("apply_edit_preview", json!({ "previewId": id })).await;
    assert_eq!(applied["path"], "happy.md");
    assert_eq!(applied["mode"], "append");
    assert_eq!(applied["diff"], preview["diff"]);
    let text = v.read("happy.md");
    assert_eq!(applied["bytesWritten"], text.len());
    assert!(text.contains("Appended"));
    assert!(PREVIEW_STORE.get(&id).is_none());
}

#[tokio::test]
async fn an_unknown_preview_id_is_a_descriptive_error() {
    let v = Vault::new();
    let err = v
        .err(
            "apply_edit_preview",
            json!({ "previewId": "prev_nonexistent" }),
        )
        .await;
    assert_eq!(
        err,
        "Preview \"prev_nonexistent\" not found or expired (TTL: 5 minutes). Re-run the original edit_note call with dryRun: true to generate a fresh preview."
    );
}

#[tokio::test]
async fn a_file_changed_since_the_preview_is_left_alone() {
    let v = Vault::new();
    v.write("changed.md", "original content\n");
    v.node("changed.md", "Changed");
    let preview = v
        .ok(
            "edit_note",
            json!({ "name": "changed.md", "mode": "append", "content": " extra", "dryRun": true }),
        )
        .await;
    v.write("changed.md", "mutated content\n");
    let err = v
        .err(
            "apply_edit_preview",
            json!({ "previewId": preview["previewId"] }),
        )
        .await;
    assert!(err.contains("has changed since the preview"), "{err}");
    assert!(err.contains("fresh preview"), "{err}");
    assert_eq!(v.read("changed.md"), "mutated content\n");
}

#[tokio::test]
async fn a_preview_applies_only_in_the_vault_that_made_it() {
    let alpha = Vault::new();
    let beta = Vault::new();
    for v in [&alpha, &beta] {
        v.ok(
            "create_note",
            json!({ "title": "Preview target", "content": "original" }),
        )
        .await;
    }
    let preview = alpha
        .ok(
            "edit_note",
            json!({ "name": "Preview target", "mode": "append", "content": "\nfrom alpha", "dryRun": true }),
        )
        .await;
    let err = beta
        .err(
            "apply_edit_preview",
            json!({ "previewId": preview["previewId"] }),
        )
        .await;
    assert!(err.contains("not found or expired"), "{err}");
    assert!(!beta.read("Preview target.md").contains("from alpha"));
    alpha
        .ok(
            "apply_edit_preview",
            json!({ "previewId": preview["previewId"] }),
        )
        .await;
    assert!(alpha.read("Preview target.md").contains("from alpha"));
}

#[tokio::test]
async fn an_applied_preview_updates_the_graph_after_the_reindex() {
    let v = Vault::new();
    v.write("Notes.md", "# Notes\n\nEmpty body.\n");
    v.write("Target.md", "# Target\n\nExists.\n");
    v.index().await;
    let preview = v
        .ok(
            "edit_note",
            json!({ "name": "Notes", "mode": "append", "content": "Now links to [[Target]].", "dryRun": true }),
        )
        .await;
    let applied = v
        .ok(
            "apply_edit_preview",
            json!({ "previewId": preview["previewId"] }),
        )
        .await;
    assert_eq!(applied["path"], "Notes.md");
    v.ctx.wait_for_background_reindex().await;
    let inbound = v
        .db()
        .with(|c| get_edges_by_target(c, "Target.md"))
        .unwrap();
    assert!(inbound.iter().any(|e| e.source_id == "Notes.md"));
}
