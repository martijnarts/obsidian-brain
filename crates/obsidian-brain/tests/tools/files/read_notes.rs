//! Ported from `test/tools/read-notes.test.ts`.

use serde_json::json;

use super::harness::{Harness, INVALID, assert_contains};

fn harness() -> Harness {
    let h = Harness::new();
    h.node("Alpha.md", "Alpha", "alpha body", json!({ "tags": ["a"] }));
    h.node("dir/Beta.md", "Beta", &"b".repeat(3000), json!({}));
    h.node("x/Gamma.md", "Gamma one", "", json!({}));
    h.node("y/Gamma.md", "Gamma two", "", json!({}));
    h
}

#[tokio::test]
async fn reads_several_notes_in_order_by_title_and_by_path() {
    let h = harness();
    let out = h
        .ok("read_notes", json!({ "names": ["Alpha", "dir/Beta.md"] }))
        .await;
    let notes = out["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 2);
    assert_eq!(
        notes[0],
        json!({
            "path": "Alpha.md",
            "title": "Alpha",
            "frontmatter": { "tags": ["a"] },
            "content": "alpha body",
            "truncated": false,
        })
    );
    assert_eq!(notes[1]["path"], "dir/Beta.md");
}

#[tokio::test]
async fn truncates_each_body_at_2000_chars_by_default() {
    let h = harness();
    let out = h.ok("read_notes", json!({ "names": ["Beta"] })).await;
    assert_eq!(out["notes"][0]["content"].as_str().unwrap().len(), 2000);
    assert_eq!(out["notes"][0]["truncated"], true);
}

#[tokio::test]
async fn honours_max_content_length_per_note() {
    let h = harness();
    let out = h
        .ok(
            "read_notes",
            json!({ "names": ["Alpha", "Beta"], "maxContentLength": 5 }),
        )
        .await;
    assert_eq!(out["notes"][0]["content"], "alpha");
    assert_eq!(out["notes"][0]["truncated"], true);
    assert_eq!(out["notes"][1]["content"], "bbbbb");
}

#[tokio::test]
async fn truncation_counts_utf16_units() {
    let h = Harness::new();
    h.node("E.md", "E", "😀😀ab", json!({}));
    let out = h
        .ok(
            "read_notes",
            json!({ "names": ["E"], "maxContentLength": 4 }),
        )
        .await;
    assert_eq!(out["notes"][0]["content"], "😀😀");
    assert_eq!(out["notes"][0]["truncated"], true);
}

#[tokio::test]
async fn a_missing_or_ambiguous_name_fails_only_its_own_entry() {
    let h = harness();
    let out = h
        .ok("read_notes", json!({ "names": ["Nope", "Alpha", "gamma"] }))
        .await;
    let notes = out["notes"].as_array().unwrap();
    assert_eq!(
        notes[0],
        json!({ "name": "Nope", "error": "No note found matching \"Nope\"" })
    );
    assert_eq!(notes[1]["path"], "Alpha.md");
    assert_eq!(notes[2]["name"], "gamma");
    assert_contains(
        notes[2]["error"].as_str().unwrap(),
        &["Multiple notes match \"gamma\""],
    );
}

#[tokio::test]
async fn rejects_an_empty_list_and_more_than_20_names() {
    let h = harness();
    assert_contains(
        &h.err("read_notes", json!({ "names": [] })).await,
        &[INVALID],
    );
    let many = vec!["Alpha"; 21];
    assert_contains(
        &h.err("read_notes", json!({ "names": many })).await,
        &[INVALID],
    );
    let out = h.ok("read_notes", json!({ "names": &many[..20] })).await;
    assert_eq!(out["notes"].as_array().unwrap().len(), 20);
    assert_contains(
        &h.err(
            "read_notes",
            json!({ "names": ["Alpha"], "maxContentLength": 0 }),
        )
        .await,
        &[INVALID],
    );
}
