use serde_json::json;

use super::{context, edge, ids, node, ok};

fn setup() -> (
    tempfile::TempDir,
    std::sync::Arc<obsidian_brain::context::VaultContext>,
) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = context(dir.path());
    node(&ctx, "a.md", "Alpha", json!({}));
    node(&ctx, "b.md", "Beta", json!({}));
    node(&ctx, "_stub/Ghost.md", "Ghost", json!({ "_stub": true }));
    edge(&ctx, "a.md", "b.md");
    edge(&ctx, "a.md", "_stub/Ghost.md");
    (dir, ctx)
}

#[tokio::test]
async fn lists_neighbours_with_hints() {
    let (_dir, ctx) = setup();
    let out = ok(
        &ctx,
        "find_connections",
        json!({ "name": "a.md", "depth": 1 }),
    )
    .await;
    assert_eq!(ids(&out["data"]), ["b.md"]);
    assert_eq!(
        out["context"],
        json!({
            "state": { "last_connections_root": "a.md", "last_connections_count": 1 },
            "next_actions": [{
                "description": "Trace path from a.md to Beta",
                "tool": "find_path_between",
                "args": { "source": "a.md", "target": "b.md" },
                "reason": "Find the shortest link chain to the furthest neighbour in this subgraph",
            }],
        })
    );
}

#[tokio::test]
async fn returns_the_subgraph() {
    let (_dir, ctx) = setup();
    let out = ok(
        &ctx,
        "find_connections",
        json!({ "name": "a.md", "depth": 2, "returnSubgraph": true }),
    )
    .await;
    assert_eq!(ids(&out["data"]["nodes"]), ["a.md", "b.md"]);
    assert_eq!(out["data"]["edges"].as_array().unwrap().len(), 1);
    assert_eq!(out["context"]["state"]["last_connections_count"], 1);
}

#[tokio::test]
async fn excludes_stub_neighbours_unless_asked() {
    let (_dir, ctx) = setup();
    let default = ok(
        &ctx,
        "find_connections",
        json!({ "name": "a.md", "depth": 1 }),
    )
    .await;
    assert!(!ids(&default["data"]).contains(&"_stub/Ghost.md"));
    let with_stubs = ok(
        &ctx,
        "find_connections",
        json!({ "name": "a.md", "depth": 1, "includeStubs": true }),
    )
    .await;
    let found = ids(&with_stubs["data"]);
    assert!(found.contains(&"b.md"));
    assert!(found.contains(&"_stub/Ghost.md"));
}

#[tokio::test]
async fn refuses_a_depth_of_zero() {
    let (_dir, ctx) = setup();
    let e = super::err(
        &ctx,
        "find_connections",
        json!({ "name": "a.md", "depth": 0 }),
    )
    .await;
    assert!(e.contains("positive"), "{e}");
}
