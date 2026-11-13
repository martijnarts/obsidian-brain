//! Tests of the `create_folder` and `delete_folder` tools.

use obsidian_brain::store::edges::{count_edges_by_source, count_edges_by_target};
use obsidian_brain::store::nodes::get_node;
use serde_json::json;

use crate::support::{INVALID, Vault, assert_contains, outside, symlink};

fn is_empty_dir(p: &std::path::Path) -> bool {
    std::fs::read_dir(p).unwrap().next().is_none()
}

#[tokio::test]
async fn create_folder_creates_a_nested_folder_chain() {
    let h = Vault::new();
    let out = h.ok("create_folder", json!({ "path": "A/B/C" })).await;
    assert_eq!(out, json!({ "path": "A/B/C", "created": true }));
    assert!(h.abs("A/B/C").is_dir());
}

#[tokio::test]
async fn create_folder_is_idempotent_and_normalises_slashes() {
    let h = Vault::new();
    h.ok("create_folder", json!({ "path": "A" })).await;
    assert_eq!(
        h.ok("create_folder", json!({ "path": "A/" })).await,
        json!({ "path": "A", "created": false })
    );
    assert_eq!(
        h.ok("create_folder", json!({ "path": "A//B\\C/" })).await,
        json!({ "path": "A/B/C", "created": true })
    );
}

#[tokio::test]
async fn create_folder_refuses_the_root_an_existing_file_and_escapes() {
    let h = Vault::new();
    h.write("file.txt", "x");
    for path in ["", "./"] {
        assert_contains(
            &h.err("create_folder", json!({ "path": path })).await,
            &["vault root"],
        );
    }
    assert_eq!(
        h.err("create_folder", json!({ "path": "file.txt" })).await,
        "A file already exists at file.txt"
    );
    for path in ["../escape", "A/../../escape", "C:/temp"] {
        assert_contains(
            &h.err("create_folder", json!({ "path": path })).await,
            &["outside the vault"],
        );
    }
    assert!(!h.path().join("../escape").exists());
}

#[tokio::test]
async fn create_folder_refuses_to_create_through_a_symlink_that_leaves_the_vault() {
    let h = Vault::new();
    let out = outside();
    symlink(out.path(), &h.abs("out"));
    assert_contains(
        &h.err("create_folder", json!({ "path": "out/new" })).await,
        &["outside the vault"],
    );
    assert!(is_empty_dir(out.path()));
    symlink(&out.path().join("missing"), &h.abs("dangling"));
    assert_contains(
        &h.err("create_folder", json!({ "path": "dangling/x" }))
            .await,
        &["outside the vault"],
    );
}

fn delete_harness() -> Vault {
    let h = Vault::new();
    h.write("Proj/A.md", "# A\n[[Keep]] [[Ghost]]\n");
    h.write("Proj/Sub/B.md", "# B\n");
    h.write("Proj/Sub/pic.png", "png");
    h.write("Keep.md", "# Keep\n[[A]]\n");
    h.node("Proj/A.md", "A", "", json!({}));
    h.node("Proj/Sub/B.md", "B", "", json!({}));
    h.node("Proj/Stale.md", "Stale", "", json!({}));
    h.node("Keep.md", "Keep", "", json!({}));
    h.node("_stub/Ghost.md", "Ghost", "", json!({ "_stub": true }));
    h.edge("Proj/A.md", "Keep.md", "");
    h.edge("Proj/A.md", "_stub/Ghost.md", "");
    h.edge("Keep.md", "Proj/A.md", "");
    h
}

fn indexed(h: &Vault, id: &str) -> bool {
    h.ctx.db.with(|c| get_node(c, id)).unwrap().is_some()
}

#[tokio::test]
async fn delete_folder_requires_confirm_true() {
    let h = delete_harness();
    for args in [
        json!({ "path": "Proj", "recursive": true }),
        json!({ "path": "Proj", "recursive": true, "confirm": false }),
    ] {
        assert_contains(&h.err("delete_folder", args).await, &[INVALID]);
    }
    assert!(h.exists("Proj"));
}

#[tokio::test]
async fn delete_folder_deletes_an_empty_folder_without_recursive() {
    let h = delete_harness();
    std::fs::create_dir(h.abs("Empty")).unwrap();
    let data = h
        .ok("delete_folder", json!({ "path": "Empty", "confirm": true }))
        .await;
    assert_eq!(
        data["deleted"],
        json!({ "files": 0, "folders": 0, "notes": 0 })
    );
    assert!(!h.exists("Empty"));
    h.settle().await;
}

#[tokio::test]
async fn delete_folder_refuses_a_non_empty_folder_unless_recursive() {
    let h = delete_harness();
    assert_eq!(
        h.err("delete_folder", json!({ "path": "Proj", "confirm": true }))
            .await,
        "Folder Proj is not empty (3 files, 1 folders). Pass recursive: true to delete it with its contents."
    );
    assert!(h.exists("Proj/A.md"));
    assert!(indexed(&h, "Proj/A.md"));
}

#[tokio::test]
async fn delete_folder_dry_run_lists_what_would_go_and_changes_nothing() {
    let h = delete_harness();
    let data = h
        .ok(
            "delete_folder",
            json!({ "path": "Proj", "confirm": true, "recursive": true, "dryRun": true }),
        )
        .await;
    assert_eq!(
        data,
        json!({
            "dryRun": true,
            "path": "Proj",
            "files": 3,
            "folders": 1,
            "notes": 3,
            "sample": ["Proj/A.md", "Proj/Sub", "Proj/Sub/B.md", "Proj/Sub/pic.png"],
            "truncated": false,
        })
    );
    assert!(h.exists("Proj/Sub/B.md"));
    assert!(indexed(&h, "Proj/A.md"));
    assert_eq!(h.reindexes(), 0);

    let data = h
        .ok(
            "delete_folder",
            json!({ "path": "Proj", "confirm": true, "dryRun": true }),
        )
        .await;
    assert_contains(data["refused"].as_str().unwrap(), &["not empty"]);
}

#[tokio::test]
async fn delete_folder_dry_run_truncates_the_sample_at_50_paths() {
    let h = delete_harness();
    for i in 0..55 {
        h.write(&format!("Big/f{i:02}.txt"), "");
    }
    let data = h
        .ok(
            "delete_folder",
            json!({ "path": "Big", "confirm": true, "recursive": true, "dryRun": true }),
        )
        .await;
    assert_eq!(data["files"], 55);
    assert_eq!(data["sample"].as_array().unwrap().len(), 50);
    assert_eq!(data["truncated"], true);
}

#[tokio::test]
async fn recursive_delete_removes_the_files_and_purges_every_note_from_the_index() {
    let h = delete_harness();
    let data = h
        .ok(
            "delete_folder",
            json!({ "path": "Proj/", "confirm": true, "recursive": true }),
        )
        .await;
    assert_eq!(data["path"], "Proj");
    assert_eq!(
        data["deleted"],
        json!({ "files": 3, "folders": 1, "notes": 3 })
    );
    assert_eq!(
        data["deletedFromIndex"],
        json!({ "nodes": 3, "edges": 2, "stubsPruned": 1 })
    );

    assert!(!h.exists("Proj"));
    assert!(h.exists("Keep.md"));
    for id in [
        "Proj/A.md",
        "Proj/Sub/B.md",
        "Proj/Stale.md",
        "_stub/Ghost.md",
    ] {
        assert!(!indexed(&h, id), "{id}");
    }
    let (out, inc) = h.ctx.db.with(|c| {
        (
            count_edges_by_source(c, "Proj/A.md").unwrap(),
            count_edges_by_target(c, "Proj/A.md").unwrap(),
        )
    });
    assert_eq!((out, inc), (0, 0));
    assert!(indexed(&h, "Keep.md"));
    assert_eq!(h.reindexes(), 1);
    h.settle().await;
}

#[tokio::test]
async fn delete_folder_refuses_the_root_obsidian_a_file_a_missing_folder_a_symlink_and_escapes() {
    let h = delete_harness();
    std::fs::create_dir_all(h.abs(".obsidian/plugins")).unwrap();
    symlink(&h.abs("Proj"), &h.abs("alias"));
    let cases = [
        (
            json!({ "path": "", "confirm": true, "recursive": true }),
            "vault root",
        ),
        (
            json!({ "path": "/", "confirm": true, "recursive": true }),
            "outside the vault",
        ),
        (
            json!({ "path": ".obsidian", "confirm": true, "recursive": true }),
            ".obsidian",
        ),
        (
            json!({ "path": ".obsidian/plugins", "confirm": true, "recursive": true }),
            ".obsidian",
        ),
        (json!({ "path": "Keep.md", "confirm": true }), "is a file"),
        (
            json!({ "path": "Nope", "confirm": true }),
            "Folder not found: \"Nope\"",
        ),
        (
            json!({ "path": "alias", "confirm": true, "recursive": true }),
            "symlink",
        ),
        (
            json!({ "path": "..", "confirm": true, "recursive": true }),
            "outside the vault",
        ),
    ];
    for (args, want) in cases {
        assert_contains(&h.err("delete_folder", args).await, &[want]);
    }
    assert!(h.exists(".obsidian/plugins"));
    assert!(h.exists("Proj/A.md"));
}
