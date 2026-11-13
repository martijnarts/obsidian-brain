//! Tests of the `create_note` tool: the path guard, the full flow and
//! writes across several vaults.

use obsidian_brain::store::edges::get_edges_by_target;
use obsidian_brain::store::nodes::get_node;
use serde_json::json;

use crate::support::Vault;

/// A vault with `A.md` and a `linked` folder that is a symlink to a folder
/// outside it.
fn guarded() -> (Vault, std::path::PathBuf) {
    let v = Vault::new();
    let outside = v.dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    v.write("A.md", "# A\n");
    std::os::unix::fs::symlink(&outside, v.path().join("linked")).unwrap();
    v.row("A.md", "A");
    (v, outside)
}

fn nothing_outside(v: &Vault, outside: &std::path::Path) {
    assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
    let mut top: Vec<String> = std::fs::read_dir(v.dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    top.sort();
    assert_eq!(top, ["outside", "vault"]);
}

#[tokio::test]
async fn create_note_refuses_paths_that_leave_the_vault() {
    let cases = [
        (
            "a `..` directory",
            json!({ "title": "X", "directory": "../../escaped" }),
        ),
        ("a `..` title", json!({ "title": "../escaped" })),
        (
            "a `..` inside the directory",
            json!({ "title": "X", "directory": "notes/../../escaped" }),
        ),
        (
            "an absolute directory",
            json!({ "title": "X", "directory": "/tmp/ob-guard-escape" }),
        ),
        (
            "a symlinked directory",
            json!({ "title": "X", "directory": "linked" }),
        ),
    ];
    for (label, mut args) in cases {
        let (v, outside) = guarded();
        args["content"] = json!("body");
        let err = v.err("create_note", args).await;
        assert!(err.contains("outside the vault"), "{label}: {err}");
        nothing_outside(&v, &outside);
        assert!(!std::path::Path::new("/tmp/ob-guard-escape").exists());
    }
}

#[tokio::test]
async fn create_note_creates_a_note_in_a_nested_folder() {
    let (v, _outside) = guarded();
    let out = v
        .ok(
            "create_note",
            json!({ "title": "Inside", "content": "body", "directory": "a/b" }),
        )
        .await;
    assert_eq!(out, json!({ "path": "a/b/Inside.md", "title": "Inside" }));
    assert!(v.exists("a/b/Inside.md"));
}

#[tokio::test]
async fn create_note_injects_the_title_and_indexes_the_note() {
    let v = Vault::new();
    v.ok(
        "create_note",
        json!({ "title": "New", "content": "Body.", "frontmatter": { "tags": ["x"] } }),
    )
    .await;
    assert_eq!(
        v.read("New.md"),
        "---\ntags:\n  - x\ntitle: New\n---\nBody.\n"
    );
    let node = v.db().with(|c| get_node(c, "New.md")).unwrap().unwrap();
    assert_eq!(node.title, "New");
}

#[tokio::test]
async fn create_note_refuses_an_empty_title() {
    let v = Vault::new();
    let err = v
        .err("create_note", json!({ "title": "", "content": "x" }))
        .await;
    assert!(err.contains("title"), "{err}");
    assert!(!v.exists(".md"));
}

#[tokio::test]
async fn create_note_migrates_the_forward_reference_stub() {
    let v = Vault::new();
    v.write("Cars.md", "# Cars\n\nI drive a [[BMW]].\n");
    v.index().await;
    assert!(
        v.db()
            .with(|c| get_node(c, "_stub/BMW.md"))
            .unwrap()
            .is_some()
    );

    let out = v
        .ok(
            "create_note",
            json!({ "title": "BMW", "content": "The car." }),
        )
        .await;
    assert_eq!(out["path"], "BMW.md");

    // Checked before any reindex runs: the migration is part of the call.
    assert!(
        v.db()
            .with(|c| get_node(c, "_stub/BMW.md"))
            .unwrap()
            .is_none()
    );
    assert!(v.db().with(|c| get_node(c, "BMW.md")).unwrap().is_some());
    let inbound = v.db().with(|c| get_edges_by_target(c, "BMW.md")).unwrap();
    assert!(inbound.iter().any(|e| e.source_id == "Cars.md"));
    v.ctx.wait_for_background_reindex().await;
}

#[tokio::test]
async fn create_note_writes_to_its_own_vault_only() {
    let alpha = Vault::new();
    let beta = Vault::new();
    alpha
        .ok(
            "create_note",
            json!({ "title": "Only in alpha", "content": "alpha body" }),
        )
        .await;
    assert!(alpha.read("Only in alpha.md").contains("alpha body"));
    assert!(!beta.exists("Only in alpha.md"));
}

#[tokio::test]
async fn create_note_refuses_an_existing_note() {
    let v = Vault::new();
    v.write("Taken.md", "keep");
    let err = v
        .err("create_note", json!({ "title": "Taken", "content": "new" }))
        .await;
    assert_eq!(err, "File already exists: Taken.md");
    assert_eq!(v.read("Taken.md"), "keep");
}
