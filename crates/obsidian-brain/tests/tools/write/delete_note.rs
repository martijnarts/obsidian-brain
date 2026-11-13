//! Ported from `test/tools/delete-note.test.ts` and the `delete_note` case
//! of `test/integration/graph-tools.test.ts`.

use obsidian_brain::store::edges::{count_edges_by_source, count_edges_by_target};
use obsidian_brain::store::nodes::get_node;
use serde_json::json;

use crate::support::Vault;

#[tokio::test]
async fn removed_edges_wrap_the_result_in_a_next_actions_envelope() {
    let v = Vault::new();
    v.write("target.md", "# Target\n");
    v.row("target.md", "Target");
    v.row("other.md", "Other");
    v.edge("target.md", "other.md", "");
    let out = v
        .ok(
            "delete_note",
            json!({ "name": "target.md", "confirm": true }),
        )
        .await;
    assert_eq!(
        out["context"],
        json!({
            "next_actions": [{
                "description": "Check for newly orphaned notes",
                "tool": "rank_notes",
                "args": { "metric": "influence", "minIncomingLinks": 0 },
                "reason": "Removed 1 edge — some notes may now be orphans",
            }],
        })
    );
    assert_eq!(out["data"]["deletedFromIndex"]["edges"], 1);
    assert_eq!(out["data"]["path"], "target.md");
    assert!(!v.exists("target.md"));
}

#[tokio::test]
async fn no_removed_edges_give_the_bare_result() {
    let v = Vault::new();
    v.write("lonely.md", "# Lonely\n");
    v.row("lonely.md", "Lonely");
    let out = v
        .ok(
            "delete_note",
            json!({ "name": "lonely.md", "confirm": true }),
        )
        .await;
    assert!(out.get("context").is_none());
    assert_eq!(
        out,
        json!({
            "path": "lonely.md",
            "deletedFromIndex": { "node": true, "edges": 0, "embedding": true, "stubsPruned": 0 },
        })
    );
}

#[tokio::test]
async fn a_dry_run_reports_without_deleting() {
    let v = Vault::new();
    v.write("victim.md", "# Victim\n");
    v.row("victim.md", "Victim");
    v.row("other.md", "Other");
    v.edge("victim.md", "other.md", "");
    v.row("_stub/Ghost.md", "Ghost");
    v.edge("victim.md", "_stub/Ghost.md", "");
    let out = v
        .ok(
            "delete_note",
            json!({ "name": "victim.md", "confirm": true, "dryRun": true }),
        )
        .await;
    assert_eq!(
        out,
        json!({
            "dryRun": true,
            "wouldDelete": { "path": "victim.md", "node": true, "edges": 2, "stubsToPrune": 1 },
        })
    );
    assert!(v.exists("victim.md"));
    assert_eq!(
        v.db()
            .with(|c| count_edges_by_source(c, "victim.md"))
            .unwrap(),
        2
    );
    assert_eq!(v.ctx.background_reindex_count(), 0);
}

#[tokio::test]
async fn confirm_must_be_true() {
    let v = Vault::new();
    v.write("keep.md", "# Keep\n");
    v.row("keep.md", "Keep");
    let err = v
        .err(
            "delete_note",
            json!({ "name": "keep.md", "confirm": false }),
        )
        .await;
    assert!(err.contains("confirm"), "{err}");
    let err = v.err("delete_note", json!({ "name": "keep.md" })).await;
    assert!(err.contains("confirm"), "{err}");
    assert!(v.exists("keep.md"));
}

#[tokio::test]
async fn deleting_a_linked_note_removes_its_inbound_edges() {
    let v = Vault::new();
    v.write("Target.md", "# Target\n\nbody\n");
    v.write("A.md", "# A\n\nSee [[Target]].\n");
    v.index().await;
    assert_eq!(
        v.db()
            .with(|c| count_edges_by_target(c, "Target.md"))
            .unwrap(),
        1
    );
    let out = v
        .ok("delete_note", json!({ "name": "Target", "confirm": true }))
        .await;
    let payload = out.get("data").unwrap_or(&out);
    assert!(payload.get("deletedFromIndex").is_some());
    assert!(v.db().with(|c| get_node(c, "Target.md")).unwrap().is_none());
    assert_eq!(
        v.db()
            .with(|c| count_edges_by_target(c, "Target.md"))
            .unwrap(),
        0
    );
    v.ctx.wait_for_background_reindex().await;
    // A.md still says [[Target]]; the reparse points it at a stub.
    assert!(v.db().with(|c| get_node(c, "A.md")).unwrap().is_some());
}
