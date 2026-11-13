use std::sync::Arc;

use obsidian_brain::context::VaultContext;
use serde_json::{Value, json};

use super::{context, err, node, ok};
use crate::support::write;

const PROJECT: &str = "---
title: Project
- [ ] not a task, frontmatter
---
# Project

- [ ] Write spec 📅 2026-10-12
  - [x] Draft outline
  - plain bullet
    - [/] Nested under a bullet
* [-] Dropped idea
1. [X] Numbered done

```md
- [ ] inside a fence
```

Paragraph text.
\t- [ ] tab-indented after a paragraph
- [?] question
- [ ]
-[ ] no space is not a task
- [ ]no space after bracket is not a task";

fn setup() -> (tempfile::TempDir, Arc<VaultContext>) {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path();
    write(vault, "Work/Project.md", PROJECT);
    write(
        vault,
        "Work/Sub/Deep.md",
        "- [ ] deep task\r\n- [x] deep done\r\n",
    );
    write(
        vault,
        "Inbox.md",
        "- [ ] inbox one\n- [ ] inbox two\n- [ ] inbox three\n",
    );
    write(vault, ".obsidian/Hidden.md", "- [ ] hidden\n");
    let ctx = context(vault);
    node(&ctx, "Work/Project.md", "Project", json!({}));
    node(&ctx, "Inbox.md", "Inbox", json!({}));
    node(&ctx, "Work/Plan A.md", "Plan A", json!({}));
    node(&ctx, "Work/Plan B.md", "Plan B", json!({}));
    (dir, ctx)
}

fn field<'a>(out: &'a Value, key: &str) -> Vec<&'a Value> {
    out["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| &t[key])
        .collect()
}

#[tokio::test]
async fn parses_every_task_shape_of_one_note() {
    let (_dir, ctx) = setup();
    let out = ok(
        &ctx,
        "list_tasks",
        json!({ "name": "Project", "status": "all" }),
    )
    .await;
    assert_eq!(out["total"], 8);
    assert_eq!(out["truncated"], false);
    let p = "Work/Project.md";
    assert_eq!(
        out["tasks"],
        json!([
            { "path": p, "line": 7, "status": " ", "state": "open", "text": "Write spec 📅 2026-10-12", "indent": 0, "due": "2026-10-12" },
            { "path": p, "line": 8, "status": "x", "state": "done", "text": "Draft outline", "indent": 1, "parentLine": 7 },
            { "path": p, "line": 10, "status": "/", "state": "open", "text": "Nested under a bullet", "indent": 2, "parentLine": 7 },
            { "path": p, "line": 11, "status": "-", "state": "cancelled", "text": "Dropped idea", "indent": 0 },
            { "path": p, "line": 12, "status": "X", "state": "done", "text": "Numbered done", "indent": 0 },
            { "path": p, "line": 19, "status": " ", "state": "open", "text": "tab-indented after a paragraph", "indent": 0 },
            { "path": p, "line": 20, "status": "?", "state": "open", "text": "question", "indent": 0 },
            { "path": p, "line": 21, "status": " ", "state": "open", "text": "", "indent": 0 },
        ])
    );
    let keys: Vec<&String> = out["tasks"][0].as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        ["path", "line", "status", "state", "text", "indent", "due"]
    );
}

#[tokio::test]
async fn defaults_to_open_and_done_picks_x() {
    let (_dir, ctx) = setup();
    let open = ok(&ctx, "list_tasks", json!({ "name": "Work/Project.md" })).await;
    assert_eq!(
        field(&open, "line"),
        [7, 10, 19, 20, 21]
            .map(|n| json!(n))
            .iter()
            .collect::<Vec<_>>()
    );
    let done = ok(
        &ctx,
        "list_tasks",
        json!({ "name": "Project", "status": "done" }),
    )
    .await;
    assert_eq!(
        field(&done, "line"),
        [8, 12].map(|n| json!(n)).iter().collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn scans_the_vault_in_path_order() {
    let (_dir, ctx) = setup();
    let out = ok(&ctx, "list_tasks", json!({ "status": "all" })).await;
    let mut paths: Vec<&str> = field(&out, "path")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    paths.dedup();
    assert_eq!(paths, ["Inbox.md", "Work/Project.md", "Work/Sub/Deep.md"]);
    let deep: Vec<&str> = out["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["path"] == "Work/Sub/Deep.md")
        .map(|t| t["text"].as_str().unwrap())
        .collect();
    assert_eq!(deep, ["deep task", "deep done"]);
}

#[tokio::test]
async fn scopes_to_a_folder() {
    let (_dir, ctx) = setup();
    for folder in ["Work/Sub", "/Work/Sub/", "./Work/Sub"] {
        let out = ok(
            &ctx,
            "list_tasks",
            json!({ "folder": folder, "status": "all" }),
        )
        .await;
        assert_eq!(
            field(&out, "path"),
            [json!("Work/Sub/Deep.md"), json!("Work/Sub/Deep.md")]
                .iter()
                .collect::<Vec<_>>()
        );
    }
    let root = ok(
        &ctx,
        "list_tasks",
        json!({ "folder": ".", "status": "all" }),
    )
    .await;
    assert_eq!(root["total"], 13);
}

#[tokio::test]
async fn pages_with_limit_and_offset() {
    let (_dir, ctx) = setup();
    let first = ok(&ctx, "list_tasks", json!({ "folder": "", "limit": 2 })).await;
    assert_eq!(first["total"], 9);
    assert_eq!(first["truncated"], true);
    assert_eq!(
        field(&first, "text"),
        [&json!("inbox one"), &json!("inbox two")]
    );
    let second = ok(&ctx, "list_tasks", json!({ "limit": 2, "offset": 2 })).await;
    assert_eq!(second["offset"], 2);
    assert_eq!(
        field(&second, "text"),
        [&json!("inbox three"), &json!("Write spec 📅 2026-10-12")]
    );
    let past = ok(&ctx, "list_tasks", json!({ "offset": 50 })).await;
    assert_eq!(past["tasks"], json!([]));
    assert_eq!(past["truncated"], false);
    assert!(
        err(&ctx, "list_tasks", json!({ "limit": 1001 }))
            .await
            .contains("Too big: expected number to be <=1000 at limit")
    );
}

#[tokio::test]
async fn rejects_name_with_folder_bad_folders_and_escapes() {
    let (_dir, ctx) = setup();
    assert!(
        err(
            &ctx,
            "list_tasks",
            json!({ "name": "Inbox", "folder": "Work" })
        )
        .await
        .contains("not both")
    );
    assert!(
        err(&ctx, "list_tasks", json!({ "folder": "Nope" }))
            .await
            .contains("Folder not found")
    );
    assert!(
        err(&ctx, "list_tasks", json!({ "folder": "Inbox.md" }))
            .await
            .contains("Folder not found")
    );
    assert!(
        err(&ctx, "list_tasks", json!({ "folder": ".obsidian" }))
            .await
            .contains("Hidden folders")
    );
    assert!(
        err(&ctx, "list_tasks", json!({ "folder": "../.." }))
            .await
            .contains("outside the vault")
    );
}

#[tokio::test]
async fn errors_on_a_missing_ambiguous_or_unreadable_note() {
    let (_dir, ctx) = setup();
    assert!(
        err(&ctx, "list_tasks", json!({ "name": "Nothing like it" }))
            .await
            .contains("No note found")
    );
    assert!(
        err(&ctx, "list_tasks", json!({ "name": "Plan" }))
            .await
            .contains("Multiple notes match")
    );
    assert!(
        err(&ctx, "list_tasks", json!({ "name": "Plan A" }))
            .await
            .contains("Could not read \"Work/Plan A.md\"")
    );
}

#[tokio::test]
async fn skips_a_symlink_out_of_the_vault() {
    let (dir, ctx) = setup();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("Secret.md"), "- [ ] secret\n").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("Secret.md"),
        dir.path().join("Linked.md"),
    )
    .unwrap();
    let out = ok(&ctx, "list_tasks", json!({ "status": "all" })).await;
    assert!(!field(&out, "text").contains(&&json!("secret")));

    node(&ctx, "Linked.md", "Linked", json!({}));
    assert!(
        err(&ctx, "list_tasks", json!({ "name": "Linked" }))
            .await
            .contains("outside the vault")
    );
}
