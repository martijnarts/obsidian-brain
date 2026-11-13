use std::sync::Arc;

use obsidian_brain::context::VaultContext;
use serde_json::{Value, json};

use super::{context, edge, ids, node, ok};

/// Hub has 3 incoming links, orphan 1, the stub 1.
fn setup() -> (tempfile::TempDir, Arc<VaultContext>) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = context(dir.path());
    node(&ctx, "hub.md", "Hub", json!({}));
    node(&ctx, "leaf1.md", "Leaf 1", json!({}));
    node(&ctx, "leaf2.md", "Leaf 2", json!({}));
    node(&ctx, "leaf3.md", "Leaf 3", json!({}));
    node(&ctx, "orphan.md", "Orphan", json!({}));
    node(&ctx, "stub.md", "Stub", json!({ "_stub": true }));
    edge(&ctx, "leaf1.md", "hub.md");
    edge(&ctx, "leaf2.md", "hub.md");
    edge(&ctx, "leaf3.md", "hub.md");
    edge(&ctx, "leaf1.md", "orphan.md");
    edge(&ctx, "leaf2.md", "stub.md");
    (dir, ctx)
}

#[tokio::test]
async fn influence_filters_nodes_with_fewer_than_two_incoming_links() {
    let (_dir, ctx) = setup();
    let out = ok(&ctx, "rank_notes", json!({ "metric": "influence" })).await;
    assert_eq!(ids(&out), ["hub.md"]);
    assert_eq!(out[0]["title"], "Hub");
}

#[tokio::test]
async fn influence_with_min_zero_includes_low_inbound_nodes() {
    let (_dir, ctx) = setup();
    let out = ok(
        &ctx,
        "rank_notes",
        json!({ "metric": "influence", "minIncomingLinks": 0 }),
    )
    .await;
    let found = ids(&out);
    assert_eq!(found[0], "hub.md");
    assert!(found.contains(&"orphan.md".into()));
}

#[tokio::test]
async fn stubs_are_excluded_unless_asked() {
    let (_dir, ctx) = setup();
    let default = ok(
        &ctx,
        "rank_notes",
        json!({ "metric": "influence", "minIncomingLinks": 0 }),
    )
    .await;
    let with_stubs = ok(
        &ctx,
        "rank_notes",
        json!({ "metric": "influence", "minIncomingLinks": 0, "includeStubs": true }),
    )
    .await;
    assert!(ids(&default).contains(&"hub.md".into()));
    assert!(ids(&with_stubs).contains(&"hub.md".into()));
    assert!(!ids(&default).contains(&"stub.md".into()));
    assert!(ids(&with_stubs).contains(&"stub.md".into()));
}

#[tokio::test]
async fn bridging_scores_are_normalized() {
    let (_dir, ctx) = setup();
    let out = ok(&ctx, "rank_notes", json!({ "metric": "bridging" })).await;
    for entry in out.as_array().unwrap() {
        let score = entry["score"].as_f64().unwrap();
        assert!((0.0..=1.0).contains(&score), "{entry}");
    }
}

#[tokio::test]
async fn the_bridge_of_a_three_node_path_scores_a_third() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = context(dir.path());
    for id in ["a.md", "b.md", "c.md"] {
        node(&ctx, id, id, json!({}));
    }
    edge(&ctx, "a.md", "b.md");
    edge(&ctx, "b.md", "c.md");
    let out = ok(&ctx, "rank_notes", json!({ "metric": "bridging" })).await;
    let b = out
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "b.md")
        .unwrap();
    assert!((b["score"].as_f64().unwrap() - 1.0 / 3.0).abs() < 1e-9);
    // Zero scores print as JSON integers, as `JSON.stringify` prints them.
    assert!(
        out.as_array()
            .unwrap()
            .iter()
            .any(|r| r["score"] == json!(0))
    );
}

#[tokio::test]
async fn both_is_the_default_and_limit_caps_each_list() {
    let (_dir, ctx) = setup();
    let out = ok(
        &ctx,
        "rank_notes",
        json!({ "limit": 1, "minIncomingLinks": 0 }),
    )
    .await;
    let keys: Vec<&String> = out.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["influence", "bridging"]);
    assert_eq!(out["influence"].as_array().unwrap().len(), 1);
    assert_eq!(out["bridging"].as_array().map(Vec::len), Some(1));
    assert!(matches!(out["bridging"][0]["score"], Value::Number(_)));
}
