//! Ported from `test/tools/create-move-path-guard.test.ts` (the
//! `move_note` half), `test/tools/move-note/dry-run.test.ts`, the full-flow
//! case of `test/tools/move-note/stub-pruning.test.ts` and the `move_note`
//! case of `test/integration/graph-tools.test.ts`. The link-rewrite cases
//! are unit tests in the tool; the `pruneOrphanStubs` cases belong to the
//! store.

use obsidian_brain::store::edges::get_edges_by_target;
use obsidian_brain::store::nodes::{all_node_ids, get_node};
use serde_json::json;

use crate::support::Vault;

#[tokio::test]
async fn move_note_refuses_destinations_outside_the_vault() {
    for (label, destination) in [
        ("a `..` destination", "../escaped.md"),
        ("a deep `..` destination", "notes/../../../escaped.md"),
        ("an absolute destination", "/tmp/ob-guard-escape.md"),
        ("a symlinked destination folder", "linked/A.md"),
    ] {
        for dry_run in [false, true] {
            let v = Vault::new();
            let outside = v.dir.path().join("outside");
            std::fs::create_dir(&outside).unwrap();
            std::os::unix::fs::symlink(&outside, v.path().join("linked")).unwrap();
            v.write("A.md", "# A\n");
            v.node("A.md", "A");
            let err = v
                .err(
                    "move_note",
                    json!({ "source": "A.md", "destination": destination, "dryRun": dry_run }),
                )
                .await;
            assert!(err.contains("outside the vault"), "{label}: {err}");
            assert!(v.exists("A.md"));
            assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
            assert!(!std::path::Path::new("/tmp/ob-guard-escape.md").exists());
        }
    }
}

#[tokio::test]
async fn move_note_moves_a_note_into_a_new_folder() {
    let v = Vault::new();
    v.write("A.md", "# A\n");
    v.node("A.md", "A");
    let out = v
        .ok(
            "move_note",
            json!({ "source": "A.md", "destination": "archive/A.md" }),
        )
        .await;
    assert_eq!(
        out,
        json!({
            "oldPath": "A.md",
            "newPath": "archive/A.md",
            "linksRewritten": { "files": 0, "occurrences": 0 },
            "stubsPruned": 0,
        })
    );
    assert!(v.exists("archive/A.md"));
    assert!(!v.exists("A.md"));
}

#[tokio::test]
async fn a_dry_run_reports_without_mutating() {
    let v = Vault::new();
    v.write("alpha.md", "# Alpha\n");
    v.write("ref.md", "See [[alpha]] for details.\n");
    v.node("alpha.md", "Alpha");
    v.node("ref.md", "Ref");
    v.edge("ref.md", "alpha.md", "link");
    let before: Vec<String> = v.db().with(all_node_ids).unwrap();
    let out = v
        .ok(
            "move_note",
            json!({ "source": "alpha.md", "destination": "beta", "dryRun": true }),
        )
        .await;
    assert_eq!(
        out,
        json!({
            "dryRun": true,
            "oldPath": "alpha.md",
            "newPath": "beta.md",
            "linksToRewrite": [{ "file": "ref.md", "occurrences": 1 }],
            "totalFiles": 1,
            "totalOccurrences": 1,
        })
    );
    assert_eq!(v.read("ref.md"), "See [[alpha]] for details.\n");
    assert!(v.exists("alpha.md"));
    assert_eq!(v.db().with(all_node_ids).unwrap(), before);
    assert_eq!(v.ctx.background_reindex_count(), 0);
}

#[tokio::test]
async fn the_old_stub_is_migrated_and_orphan_stubs_are_pruned() {
    let v = Vault::new();
    v.write("_move_target.md", "# Move Target\n");
    v.write(
        "_move_source.md",
        "See [[_move_target#Section A]] for context.\n",
    );
    v.node("_move_target.md", "_move_target");
    v.node("_move_source.md", "_move_source");
    v.node("_stub/_move_target.md", "_move_target");
    v.node("_stub/_move_target#Section A.md", "_move_target#Section A");
    v.node("_stub/_move_target#Orphan.md", "_move_target#Orphan");
    v.edge("_move_source.md", "_stub/_move_target.md", "link");
    v.edge("_move_source.md", "_stub/_move_target#Section A.md", "link");

    let out = v
        .ok(
            "move_note",
            json!({ "source": "_move_target.md", "destination": "_move_target_renamed.md" }),
        )
        .await;
    assert_eq!(
        out["linksRewritten"],
        json!({ "files": 1, "occurrences": 1 })
    );
    assert_eq!(out["stubsPruned"], 1);
    assert_eq!(
        v.read("_move_source.md"),
        "See [[_move_target_renamed#Section A]] for context.\n"
    );
    let ids = v.db().with(all_node_ids).unwrap();
    assert!(!ids.contains(&"_stub/_move_target.md".to_string()));
    assert!(!ids.contains(&"_stub/_move_target#Orphan.md".to_string()));
    let inbound = v
        .db()
        .with(|c| get_edges_by_target(c, "_move_target_renamed.md"))
        .unwrap();
    assert_eq!(inbound.len(), 1);
    assert_eq!(inbound[0].source_id, "_move_source.md");
}

#[tokio::test]
async fn a_rename_keeps_inbound_edges_through_the_handler_and_reindex() {
    let v = Vault::new();
    v.write("Target.md", "# Target\n\nhello\n");
    v.write("A.md", "# A\n\nSee [[Target]].\n");
    v.write("B.md", "# B\n\nAlso [[Target]].\n");
    v.index().await;
    assert_eq!(
        v.db()
            .with(|c| get_edges_by_target(c, "Target.md"))
            .unwrap()
            .len(),
        2
    );
    v.ok(
        "move_note",
        json!({ "source": "Target", "destination": "Renamed & Archived" }),
    )
    .await;
    let check = |v: &Vault| {
        assert!(
            v.db()
                .with(|c| get_edges_by_target(c, "Target.md"))
                .unwrap()
                .is_empty()
        );
        let inbound = v
            .db()
            .with(|c| get_edges_by_target(c, "Renamed & Archived.md"))
            .unwrap();
        let mut sources: Vec<_> = inbound.iter().map(|e| e.source_id.clone()).collect();
        sources.sort();
        assert_eq!(sources, ["A.md", "B.md"]);
    };
    check(&v);
    assert!(v.read("A.md").contains("[[Renamed & Archived]]"));
    assert!(v.read("B.md").contains("[[Renamed & Archived]]"));
    v.ctx.wait_for_background_reindex().await;
    check(&v);
    assert!(
        v.db()
            .with(|c| get_node(c, "Renamed & Archived.md"))
            .unwrap()
            .is_some()
    );
}
