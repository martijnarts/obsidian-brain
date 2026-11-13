//! Ported from `test/tools/list-bookmarks.test.ts`.

use serde_json::{Value, json};

use crate::support::{Vault, assert_contains};

fn write_bookmarks(h: &Vault, content: &Value) {
    let text = match content {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    h.write(".obsidian/bookmarks.json", text);
}

fn sample() -> Value {
    json!({
        "items": [
            { "type": "file", "ctime": 1, "path": "Home.md", "title": "Start" },
            { "type": "search", "ctime": 2, "query": "tag:#todo" },
            {
                "type": "group",
                "ctime": 3,
                "title": "Work",
                "items": [
                    { "type": "folder", "ctime": 4, "path": "Projects" },
                    { "type": "heading", "ctime": 5, "path": "Plan.md", "subpath": "#Goals" },
                    {
                        "type": "group",
                        "ctime": 6,
                        "title": "Deep",
                        "items": [{ "type": "block", "ctime": 7, "path": "Log.md", "subpath": "#^abc" }],
                    },
                ],
            },
            { "type": "graph", "ctime": 8 },
            "junk",
        ],
    })
}

#[tokio::test]
async fn returns_an_empty_list_with_a_note_when_the_file_is_absent() {
    let h = Vault::new();
    let out = h.ok("list_bookmarks", json!({})).await;
    assert_eq!(out["items"], json!([]));
    assert_eq!(out["totalItems"], 0);
    assert_contains(out["note"].as_str().unwrap(), &["No bookmarks file"]);
}

#[tokio::test]
async fn returns_an_empty_list_with_a_note_when_the_file_is_malformed() {
    let h = Vault::new();
    for content in [json!("{not json"), json!({ "nope": true }), json!("null")] {
        write_bookmarks(&h, &content);
        let out = h.ok("list_bookmarks", json!({})).await;
        assert_contains(out["note"].as_str().unwrap(), &["not valid"]);
    }
}

#[tokio::test]
async fn returns_the_hierarchy_with_known_fields_only_and_drops_unknown_types() {
    let h = Vault::new();
    write_bookmarks(&h, &sample());
    let out = h.ok("list_bookmarks", json!({})).await;
    assert!(out.get("note").is_none());
    assert_eq!(out["totalItems"], 5);
    assert_eq!(
        out["items"],
        json!([
            { "type": "file", "path": "Home.md", "title": "Start" },
            { "type": "search", "query": "tag:#todo" },
            {
                "type": "group",
                "title": "Work",
                "items": [
                    { "type": "folder", "path": "Projects" },
                    { "type": "heading", "path": "Plan.md", "subpath": "#Goals" },
                    {
                        "type": "group",
                        "title": "Deep",
                        "items": [{ "type": "block", "path": "Log.md", "subpath": "#^abc" }],
                    },
                ],
            },
        ])
    );
    // Key order follows the TS: type, title, path, query, subpath, items.
    let text = serde_json::to_string(&out["items"][0]).unwrap();
    assert_eq!(text, r#"{"type":"file","title":"Start","path":"Home.md"}"#);
}

#[tokio::test]
async fn types_without_group_flattens_group_contents() {
    let h = Vault::new();
    write_bookmarks(&h, &sample());
    let out = h
        .ok("list_bookmarks", json!({ "types": ["file", "block"] }))
        .await;
    assert_eq!(
        out["items"],
        json!([
            { "type": "file", "path": "Home.md", "title": "Start" },
            { "type": "block", "path": "Log.md", "subpath": "#^abc" },
        ])
    );
    assert_eq!(out["totalItems"], 2);
}

#[tokio::test]
async fn types_with_group_keeps_groups_and_filters_their_children() {
    let h = Vault::new();
    write_bookmarks(&h, &sample());
    let out = h
        .ok("list_bookmarks", json!({ "types": ["group", "folder"] }))
        .await;
    assert_eq!(
        out["items"],
        json!([{
            "type": "group",
            "title": "Work",
            "items": [{ "type": "folder", "path": "Projects" }, { "type": "group", "title": "Deep", "items": [] }],
        }])
    );
    assert_eq!(out["totalItems"], 1);
}

#[tokio::test]
async fn a_group_without_items_gets_an_empty_items_array() {
    let h = Vault::new();
    write_bookmarks(
        &h,
        &json!({ "items": [{ "type": "group", "title": "Empty" }] }),
    );
    let out = h.ok("list_bookmarks", json!({})).await;
    assert_eq!(
        out["items"],
        json!([{ "type": "group", "title": "Empty", "items": [] }])
    );
}
