//! Ported from `test/tools/list-tags.test.ts`.

use serde_json::{Value, json};

use crate::support::Vault;

fn harness() -> Vault {
    let h = Vault::new();
    h.node(
        "a.md",
        "a",
        "",
        json!({ "tags": ["project/alpha", "#idea"] }),
    );
    h.node(
        "b.md",
        "b",
        "",
        json!({ "tag": "idea", "inline_tags": ["project/beta"] }),
    );
    h.node("c.md", "c", "", json!({ "tags": "idea, zeta" }));
    h.node(
        "_stub/s.md",
        "s",
        "",
        json!({ "_stub": true, "tags": ["ghost"] }),
    );
    h
}

fn names(out: &Value) -> Vec<&str> {
    out["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["tag"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn counts_notes_per_tag_parents_included_stubs_skipped() {
    let h = harness();
    let out = h.ok("list_tags", json!({})).await;
    assert_eq!(
        out["tags"],
        json!([
            { "tag": "idea", "count": 3 },
            { "tag": "project", "count": 2 },
            { "tag": "project/alpha", "count": 1 },
            { "tag": "project/beta", "count": 1 },
            { "tag": "zeta", "count": 1 },
        ])
    );
    assert_eq!(out["totalTags"], 5);
    assert_eq!(out["truncated"], false);
}

#[tokio::test]
async fn include_parents_false_counts_nested_tags_only_under_their_own_entry() {
    let h = harness();
    let out = h.ok("list_tags", json!({ "includeParents": false })).await;
    assert!(!names(&out).contains(&"project"));
    assert_eq!(out["totalTags"], 4);
}

#[tokio::test]
async fn sort_name_orders_alphabetically() {
    let h = harness();
    let out = h.ok("list_tags", json!({ "sort": "name" })).await;
    assert_eq!(
        names(&out),
        ["idea", "project", "project/alpha", "project/beta", "zeta"]
    );
}

#[tokio::test]
async fn prefix_filters_tags_with_or_without_hash() {
    let h = harness();
    let out = h.ok("list_tags", json!({ "prefix": "project/" })).await;
    assert_eq!(names(&out), ["project/alpha", "project/beta"]);
    let out = h.ok("list_tags", json!({ "prefix": "#zet" })).await;
    assert_eq!(out["tags"], json!([{ "tag": "zeta", "count": 1 }]));
}

#[tokio::test]
async fn limit_truncates_and_reports_it() {
    let h = harness();
    let out = h.ok("list_tags", json!({ "limit": 2 })).await;
    assert_eq!(out["tags"].as_array().unwrap().len(), 2);
    assert_eq!(out["totalTags"], 5);
    assert_eq!(out["truncated"], true);
}

#[tokio::test]
async fn returns_an_empty_list_for_a_vault_without_tags() {
    let h = Vault::new();
    assert_eq!(
        h.ok("list_tags", json!({})).await,
        json!({ "totalTags": 0, "truncated": false, "tags": [] })
    );
}
