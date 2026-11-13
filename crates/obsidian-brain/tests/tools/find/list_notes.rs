use obsidian_brain::store::sync::set_sync_mtime;
use serde_json::{Value, json};

use crate::support::{Vault, fixture, ids};

/// Three folders, one stub and one untagged leaf, so every filter has
/// something to tell apart.
fn notes() -> Vault {
    let f = fixture(&[]);
    f.upsert(
        "People/Alice.md",
        "Alice",
        json!({"tags": ["friend", "researcher"]}),
    );
    f.upsert("People/Bob.md", "Bob", json!({"tags": ["friend"]}));
    f.upsert("Concepts/Widget.md", "Widget", json!({"tags": ["idea"]}));
    f.upsert("orphan.md", "Orphan", json!({}));
    f.upsert(
        "_stub/Missing.md",
        "Missing",
        json!({"_stub": true, "tags": ["idea"]}),
    );
    f
}

async fn list(f: &Vault, args: Value) -> Vec<String> {
    ids(&f.ok("list_notes", args).await)
}

#[tokio::test]
async fn returns_every_node_by_default_stubs_included() {
    let f = notes();
    let ids = list(&f, json!({})).await;
    for id in [
        "People/Alice.md",
        "People/Bob.md",
        "Concepts/Widget.md",
        "orphan.md",
        "_stub/Missing.md",
    ] {
        assert!(ids.contains(&id.to_string()), "{id}");
    }
}

#[tokio::test]
async fn directory_restricts_to_a_prefix() {
    let f = notes();
    let ids = list(&f, json!({"directory": "People"})).await;
    assert_eq!(ids, ["People/Alice.md", "People/Bob.md"]);
}

#[tokio::test]
async fn tag_returns_notes_with_that_tag() {
    let f = notes();
    let ids = list(&f, json!({"tag": "idea"})).await;
    assert_eq!(ids, ["Concepts/Widget.md", "_stub/Missing.md"]);
}

#[tokio::test]
async fn include_stubs_false_drops_stubs() {
    let f = notes();
    let ids = list(&f, json!({"includeStubs": false})).await;
    assert!(!ids.contains(&"_stub/Missing.md".to_string()));
    assert!(ids.contains(&"People/Alice.md".to_string()));
}

#[tokio::test]
async fn limit_caps_the_results() {
    let f = notes();
    assert_eq!(list(&f, json!({"limit": 2})).await.len(), 2);
    assert!(!f.accepts("list_notes", json!({"limit": 0})).await);
}

#[tokio::test]
async fn a_missing_or_non_array_tags_key_reports_an_empty_list() {
    let f = notes();
    f.upsert("str.md", "Str", json!({"tags": "x"}));
    let out = f.ok("list_notes", json!({})).await;
    for id in ["orphan.md", "str.md"] {
        let row = out
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .unwrap();
        assert_eq!(row["tags"], json!([]));
    }
}

#[tokio::test]
async fn results_are_sorted_by_path() {
    let f = notes();
    let ids = list(&f, json!({})).await;
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted);
}

#[tokio::test]
async fn tag_matches_nested_inline_string_and_hash_tags() {
    let f = notes();
    f.upsert("n/nested.md", "Nested", json!({"tags": ["idea/big"]}));
    f.upsert("n/inline.md", "Inline", json!({"inline_tags": ["idea"]}));
    f.upsert("n/str.md", "Str", json!({"tags": "#idea"}));
    f.upsert("n/other.md", "Other", json!({"tags": ["ideas"]}));
    let ids = list(&f, json!({"tag": "idea", "directory": "n"})).await;
    assert_eq!(ids, ["n/inline.md", "n/nested.md", "n/str.md"]);
    assert_eq!(list(&f, json!({"tag": "#idea/big"})).await, ["n/nested.md"]);
}

#[tokio::test]
async fn sort_by_mtime_is_newest_first_with_unindexed_notes_last() {
    let f = notes();
    f.db().with(|c| {
        set_sync_mtime(c, "People/Bob.md", 3_000.0, None).unwrap();
        set_sync_mtime(c, "orphan.md", 2_000.0, None).unwrap();
        set_sync_mtime(c, "People/Alice.md", 1_000.0, None).unwrap();
    });
    let out = f
        .ok(
            "list_notes",
            json!({"sortBy": "mtime", "includeStubs": false}),
        )
        .await;
    assert_eq!(
        ids(&out),
        [
            "People/Bob.md",
            "orphan.md",
            "People/Alice.md",
            "Concepts/Widget.md"
        ]
    );
    assert_eq!(out[0]["mtime"], "1970-01-01T00:00:03.000Z");
    assert!(out[3].get("mtime").is_none());

    let top = list(&f, json!({"sortBy": "mtime", "limit": 1})).await;
    assert_eq!(top, ["People/Bob.md"]);
}

#[tokio::test]
async fn directory_and_tag_intersect() {
    let f = notes();
    let ids = list(&f, json!({"directory": "People", "tag": "researcher"})).await;
    assert_eq!(ids, ["People/Alice.md"]);
}
