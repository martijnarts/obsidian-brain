//! Ported from `test/tools/list-property-values.test.ts`.

use serde_json::json;

use crate::support::{INVALID, Vault, assert_contains};

fn harness() -> Vault {
    let h = Vault::new();
    h.node(
        "Work/a.md",
        "a",
        "",
        json!({ "status": "active", "area": ["x", "y"] }),
    );
    h.node(
        "Work/b.md",
        "b",
        "",
        json!({ "status": "done", "area": ["x"] }),
    );
    h.node(
        "Home/c.md",
        "c",
        "",
        json!({ "status": "active", "priority": 5 }),
    );
    h.node(
        "Home/d.md",
        "d",
        "",
        json!({ "priority": "5", "status": null }),
    );
    h.node("e.md", "e", "", json!({}));
    h.node(
        "_stub/s.md",
        "s",
        "",
        json!({ "_stub": true, "status": "ghost" }),
    );
    h
}

#[tokio::test]
async fn counts_distinct_values_by_count_desc_and_reports_notes_with_key() {
    let h = harness();
    let out = h
        .ok("list_property_values", json!({ "key": "status" }))
        .await;
    assert_eq!(out["key"], "status");
    assert_eq!(out["notesWithKey"], 4);
    assert_eq!(
        out["values"],
        json!([
            { "value": "active", "count": 2 },
            { "value": "done", "count": 1 },
            { "value": null, "count": 1 },
        ])
    );
    assert_eq!(out["totalDistinct"], 3);
    assert_eq!(out["truncated"], false);
}

#[tokio::test]
async fn counts_each_element_of_a_list_value() {
    let h = harness();
    let out = h.ok("list_property_values", json!({ "key": "area" })).await;
    assert_eq!(out["notesWithKey"], 2);
    assert_eq!(
        out["values"],
        json!([{ "value": "x", "count": 2 }, { "value": "y", "count": 1 }])
    );
}

#[tokio::test]
async fn keeps_a_number_and_a_string_with_the_same_text_distinct() {
    let h = harness();
    let out = h
        .ok("list_property_values", json!({ "key": "priority" }))
        .await;
    let values: Vec<_> = out["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["value"].clone())
        .collect();
    assert_eq!(values.len(), 2);
    assert!(values.contains(&json!(5)));
    assert!(values.contains(&json!("5")));
}

#[tokio::test]
async fn folder_restricts_the_scan_with_or_without_a_trailing_slash() {
    let h = harness();
    for folder in ["Work", "Work/"] {
        let out = h
            .ok(
                "list_property_values",
                json!({ "key": "status", "folder": folder }),
            )
            .await;
        assert_eq!(out["notesWithKey"], 2);
        assert_eq!(
            out["values"],
            json!([{ "value": "active", "count": 1 }, { "value": "done", "count": 1 }])
        );
    }
}

#[tokio::test]
async fn limit_truncates_and_keeps_total_distinct() {
    let h = harness();
    let out = h
        .ok(
            "list_property_values",
            json!({ "key": "status", "limit": 1 }),
        )
        .await;
    assert_eq!(out["values"], json!([{ "value": "active", "count": 2 }]));
    assert_eq!(out["totalDistinct"], 3);
    assert_eq!(out["truncated"], true);
}

#[tokio::test]
async fn returns_an_empty_result_for_an_unknown_key() {
    let h = harness();
    assert_eq!(
        h.ok("list_property_values", json!({ "key": "nope" })).await,
        json!({
            "key": "nope",
            "notesWithKey": 0,
            "totalDistinct": 0,
            "truncated": false,
            "values": [],
        })
    );
    assert_contains(
        &h.err("list_property_values", json!({ "key": "" })).await,
        &[INVALID],
    );
}
