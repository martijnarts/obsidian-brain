use std::sync::Arc;

use obsidian_brain::context::VaultContext;
use serde_json::json;

use super::{context, err, node, ok};

const NOTE: &str = "---
tags: [todo]
---
# Todo

- [ ] Buy milk  
\t* [x] Call 🐙 back
3. [/] Half done
Plain line
```
- [ ] fenced
```
";

fn setup(content: &str) -> (tempfile::TempDir, Arc<VaultContext>) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Todo.md"), content).unwrap();
    let ctx = context(dir.path());
    node(&ctx, "Todo.md", "Todo", json!({}));
    (dir, ctx)
}

fn read(dir: &tempfile::TempDir) -> String {
    std::fs::read_to_string(dir.path().join("Todo.md")).unwrap()
}

#[tokio::test]
async fn done_ticks_the_box_and_nothing_else() {
    let (dir, ctx) = setup(NOTE);
    let out = ok(
        &ctx,
        "set_task_status",
        json!({ "name": "Todo", "line": 6, "status": "done" }),
    )
    .await;
    assert_eq!(
        out,
        json!({
            "path": "Todo.md", "line": 6, "previousStatus": " ", "status": "x",
            "state": "done", "text": "Buy milk", "changed": true,
        })
    );
    assert_eq!(
        read(&dir),
        NOTE.replace("- [ ] Buy milk  ", "- [x] Buy milk  ")
    );
    assert_eq!(ctx.background_reindex_count(), 1);
    ctx.wait_for_background_reindex().await;
    let entries: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    assert_eq!(entries, ["Todo.md"]);
}

#[tokio::test]
async fn open_unticks_a_tab_indented_star_task() {
    let (dir, ctx) = setup(NOTE);
    ok(
        &ctx,
        "set_task_status",
        json!({ "name": "Todo", "line": 7, "status": "open" }),
    )
    .await;
    assert_eq!(read(&dir), NOTE.replace("\t* [x] Call", "\t* [ ] Call"));
}

#[tokio::test]
async fn writes_a_raw_status_character() {
    let (dir, ctx) = setup(NOTE);
    let out = ok(
        &ctx,
        "set_task_status",
        json!({ "name": "Todo.md", "line": 8, "status": "-" }),
    )
    .await;
    assert_eq!(out["state"], "cancelled");
    assert_eq!(out["previousStatus"], "/");
    assert_eq!(read(&dir), NOTE.replace("3. [/]", "3. [-]"));
    ok(
        &ctx,
        "set_task_status",
        json!({ "name": "Todo", "line": 8, "status": "🔥" }),
    )
    .await;
    assert_eq!(read(&dir), NOTE.replace("3. [/]", "3. [🔥]"));
}

#[tokio::test]
async fn keeps_crlf_line_endings() {
    let crlf = NOTE.replace('\n', "\r\n");
    let (dir, ctx) = setup(&crlf);
    ok(
        &ctx,
        "set_task_status",
        json!({ "name": "Todo", "line": 6, "status": "done" }),
    )
    .await;
    assert_eq!(read(&dir), crlf.replace("- [ ] Buy", "- [x] Buy"));
}

#[tokio::test]
async fn does_not_write_an_unchanged_status() {
    let (dir, ctx) = setup(NOTE);
    let out = ok(
        &ctx,
        "set_task_status",
        json!({ "name": "Todo", "line": 7, "status": "x" }),
    )
    .await;
    assert_eq!(out["changed"], false);
    assert_eq!(ctx.background_reindex_count(), 0);
    assert_eq!(read(&dir), NOTE);
}

#[tokio::test]
async fn expected_text_guards_against_a_stale_line() {
    let (dir, ctx) = setup(NOTE);
    ok(
        &ctx,
        "set_task_status",
        json!({ "name": "Todo", "line": 6, "status": "done", "expectedText": " Buy milk " }),
    )
    .await;
    let e = err(
        &ctx,
        "set_task_status",
        json!({ "name": "Todo", "line": 7, "status": "open", "expectedText": "Buy milk" }),
    )
    .await;
    assert!(e.contains(r#"is "Call 🐙 back", not "Buy milk""#), "{e}");
    assert_eq!(read(&dir), NOTE.replace("- [ ] Buy", "- [x] Buy"));
}

#[tokio::test]
async fn refuses_non_task_lines_and_lines_past_the_end() {
    let (dir, ctx) = setup(NOTE);
    for line in [2, 4, 9, 11] {
        let e = err(
            &ctx,
            "set_task_status",
            json!({ "name": "Todo", "line": line, "status": "done" }),
        )
        .await;
        assert!(
            e.contains(&format!("Line {line} of \"Todo.md\" is not a task")),
            "{e}"
        );
    }
    let e = err(
        &ctx,
        "set_task_status",
        json!({ "name": "Todo", "line": 9, "status": "done" }),
    )
    .await;
    assert!(
        e.ends_with(r#"is not a task: "Plain line". Use list_tasks to find the line."#),
        "{e}"
    );
    let past = err(
        &ctx,
        "set_task_status",
        json!({ "name": "Todo", "line": 99, "status": "done" }),
    )
    .await;
    assert!(
        past.contains("past the end") && past.contains("13 lines"),
        "{past}"
    );
    assert_eq!(read(&dir), NOTE);
}

#[tokio::test]
async fn rejects_a_bad_status() {
    let (_dir, ctx) = setup(NOTE);
    for status in ["", "xx", "]", "[", "\n"] {
        let e = err(
            &ctx,
            "set_task_status",
            json!({ "name": "Todo", "line": 6, "status": status }),
        )
        .await;
        assert!(e.contains("status must be"), "{e}");
    }
}

#[tokio::test]
async fn errors_on_a_missing_note() {
    let (_dir, ctx) = setup(NOTE);
    let e = err(
        &ctx,
        "set_task_status",
        json!({ "name": "Nope", "line": 1, "status": "done" }),
    )
    .await;
    assert!(e.contains("No note found"), "{e}");
}
