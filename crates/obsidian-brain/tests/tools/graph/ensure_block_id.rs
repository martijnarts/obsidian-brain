use std::sync::Arc;

use obsidian_brain::context::VaultContext;
use regex::Regex;
use serde_json::{Value, json};

use super::{context, err, node, ok};

const NOTE: &str = "---
title: Notes
---
# Intro

First line
second line.

- item one
- item two ^two

| a | b |
|---|---|
| 1 | 2 |
After table.

```js
const x = 1;
```

> [!note]
> callout body

^callout

## Twice
## Twice
Last line";

fn setup() -> (tempfile::TempDir, Arc<VaultContext>) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Notes.md"), NOTE).unwrap();
    let ctx = context(dir.path());
    node(&ctx, "Notes.md", "Notes", json!({}));
    (dir, ctx)
}

fn text(dir: &tempfile::TempDir, rel: &str) -> String {
    std::fs::read_to_string(dir.path().join(rel)).unwrap()
}

fn lines(dir: &tempfile::TempDir) -> Vec<String> {
    text(dir, "Notes.md")
        .split('\n')
        .map(String::from)
        .collect()
}

fn assert_subset(got: &Value, want: Value) {
    for (k, v) in want.as_object().unwrap() {
        assert_eq!(&got[k], v, "key {k} of {got}");
    }
}

#[tokio::test]
async fn appends_the_id_to_the_last_line_of_a_paragraph() {
    let (dir, ctx) = setup();
    let out = ok(
        &ctx,
        "ensure_block_id",
        json!({ "name": "Notes", "line": 6, "id": "intro-para" }),
    )
    .await;
    assert_subset(
        &out,
        json!({ "path": "Notes.md", "id": "intro-para", "created": true, "dryRun": false, "line": 7, "link": "[[Notes#^intro-para]]" }),
    );
    assert_eq!(
        out["diff"],
        "Index: Notes.md\n\
         ===================================================================\n\
         --- Notes.md\toriginal\n\
         +++ Notes.md\tproposed\n\
         @@ -3,9 +3,9 @@\n ---\n # Intro\n \n First line\n-second line.\n+second line. ^intro-para\n \n - item one\n - item two ^two\n \n"
    );
    let after = lines(&dir);
    assert_eq!(after[5], "First line");
    assert_eq!(after[6], "second line. ^intro-para");
    assert_eq!(after.len(), NOTE.split('\n').count());
    assert_eq!(ctx.background_reindex_count(), 1);
    ctx.wait_for_background_reindex().await;
}

#[tokio::test]
async fn generates_a_six_character_id_on_a_list_item() {
    let (dir, ctx) = setup();
    let out = ok(
        &ctx,
        "ensure_block_id",
        json!({ "name": "Notes", "line": 9 }),
    )
    .await;
    let id = out["id"].as_str().unwrap();
    assert!(Regex::new("^[a-z0-9]{6}$").unwrap().is_match(id), "{id}");
    assert_eq!(lines(&dir)[8], format!("- item one ^{id}"));
}

#[tokio::test]
async fn returns_an_existing_id_without_writing() {
    let (dir, ctx) = setup();
    let out = ok(
        &ctx,
        "ensure_block_id",
        json!({ "name": "Notes", "line": 10, "id": "other" }),
    )
    .await;
    assert_subset(
        &out,
        json!({ "id": "two", "created": false, "line": 10, "requestedIdIgnored": "other" }),
    );
    let own = ok(
        &ctx,
        "ensure_block_id",
        json!({ "name": "Notes", "line": 22 }),
    )
    .await;
    assert_subset(
        &own,
        json!({ "id": "callout", "created": false, "line": 24 }),
    );
    assert!(own.get("requestedIdIgnored").is_none());
    assert_eq!(text(&dir, "Notes.md"), NOTE);
    assert_eq!(ctx.background_reindex_count(), 0);
}

#[tokio::test]
async fn puts_a_table_id_on_its_own_line() {
    let (dir, ctx) = setup();
    let out = ok(
        &ctx,
        "ensure_block_id",
        json!({ "name": "Notes", "line": 13, "id": "tbl" }),
    )
    .await;
    assert_eq!(out["line"], 16);
    assert_eq!(
        lines(&dir)[13..18],
        ["| 1 | 2 |", "", "^tbl", "", "After table."]
    );
}

#[tokio::test]
async fn puts_a_code_block_id_after_the_closing_fence() {
    let (dir, ctx) = setup();
    let out = ok(
        &ctx,
        "ensure_block_id",
        json!({ "name": "Notes", "line": 17, "id": "code" }),
    )
    .await;
    assert_eq!(out["line"], 21);
    assert_eq!(lines(&dir)[18..22], ["```", "", "^code", ""]);
}

#[tokio::test]
async fn targets_a_heading_by_its_text() {
    let (dir, ctx) = setup();
    let out = ok(
        &ctx,
        "ensure_block_id",
        json!({ "name": "Notes", "heading": "# Intro", "id": "top" }),
    )
    .await;
    assert_subset(&out, json!({ "line": 4, "link": "[[Notes#^top]]" }));
    assert_eq!(lines(&dir)[3], "# Intro ^top");
    let again = ok(
        &ctx,
        "ensure_block_id",
        json!({ "name": "Notes", "heading": "Intro" }),
    )
    .await;
    assert_subset(&again, json!({ "id": "top", "created": false }));
}

#[tokio::test]
async fn dry_run_leaves_the_file_alone() {
    let (dir, ctx) = setup();
    let out = ok(
        &ctx,
        "ensure_block_id",
        json!({ "name": "Notes", "line": 28, "id": "end", "dryRun": true }),
    )
    .await;
    assert_subset(
        &out,
        json!({ "id": "end", "created": true, "dryRun": true, "line": 28 }),
    );
    let diff = out["diff"].as_str().unwrap();
    assert!(diff.contains("+Last line ^end"));
    assert!(diff.ends_with(
        "-Last line\n\\ No newline at end of file\n+Last line ^end\n\\ No newline at end of file\n"
    ));
    assert_eq!(text(&dir, "Notes.md"), NOTE);
    assert_eq!(ctx.background_reindex_count(), 0);
}

#[tokio::test]
async fn refuses_bad_targets_and_ids() {
    let (dir, ctx) = setup();
    let cases = [
        (json!({}), "exactly one of `line` or `heading`"),
        (json!({ "line": 4, "heading": "Intro" }), "exactly one"),
        (json!({ "line": 6, "id": "bad id" }), "invalid"),
        (json!({ "line": 6, "id": "two" }), "^two is already used"),
        (json!({ "line": 2 }), "frontmatter"),
        (json!({ "line": 5 }), "blank"),
        (json!({ "line": 99 }), "past the end"),
        (json!({ "heading": "Missing" }), "No heading \"Missing\""),
        (
            json!({ "heading": "Twice" }),
            "appears 2 times in \"Notes.md\" (lines 26, 27)",
        ),
    ];
    for (extra, want) in cases {
        let mut args = json!({ "name": "Notes" });
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let e = err(&ctx, "ensure_block_id", args.clone()).await;
        assert!(e.contains(want), "{args}: {e}");
    }
    assert!(
        err(
            &ctx,
            "ensure_block_id",
            json!({ "name": "Nope", "line": 1 })
        )
        .await
        .contains("No note found")
    );
    assert_eq!(text(&dir, "Notes.md"), NOTE);
}

#[tokio::test]
async fn links_by_path_when_another_note_shares_the_basename() {
    let (dir, ctx) = setup();
    std::fs::create_dir(dir.path().join("Archive")).unwrap();
    std::fs::write(dir.path().join("Archive/Notes.md"), "Old text\n").unwrap();
    node(&ctx, "Archive/Notes.md", "Old notes", json!({}));
    let out = ok(
        &ctx,
        "ensure_block_id",
        json!({ "name": "Archive/Notes.md", "line": 1, "id": "old" }),
    )
    .await;
    assert_eq!(out["link"], "[[Archive/Notes#^old]]");
    assert_eq!(text(&dir, "Archive/Notes.md"), "Old text ^old\n");
}

#[tokio::test]
async fn keeps_crlf_endings() {
    let (dir, ctx) = setup();
    std::fs::write(dir.path().join("Notes.md"), NOTE.replace('\n', "\r\n")).unwrap();
    ok(
        &ctx,
        "ensure_block_id",
        json!({ "name": "Notes", "line": 6, "id": "p" }),
    )
    .await;
    ok(
        &ctx,
        "ensure_block_id",
        json!({ "name": "Notes", "line": 18, "id": "c" }),
    )
    .await;
    let t = text(&dir, "Notes.md");
    assert!(t.contains("second line. ^p\r\n"));
    assert!(t.contains("```\r\n\r\n^c\r\n\r\n"));
    assert!(!t.replace("\r\n", "").contains('\n'));
}

#[tokio::test]
async fn adds_an_own_line_id_at_the_end_of_a_file_without_a_final_newline() {
    let (dir, ctx) = setup();
    std::fs::write(
        dir.path().join("Notes.md"),
        "Intro\r\n\r\n> quote one\r\n> quote two",
    )
    .unwrap();
    let out = ok(
        &ctx,
        "ensure_block_id",
        json!({ "name": "Notes", "line": 3, "id": "q" }),
    )
    .await;
    assert_eq!(out["line"], 6);
    assert_eq!(
        text(&dir, "Notes.md"),
        "Intro\r\n\r\n> quote one\r\n> quote two\r\n\r\n^q"
    );
}
