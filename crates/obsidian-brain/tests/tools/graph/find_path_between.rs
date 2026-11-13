use serde_json::json;

use super::{context, edge, ids, node, ok};

#[tokio::test]
async fn finds_the_real_path_with_or_without_stubs() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = context(dir.path());
    node(&ctx, "a.md", "A", json!({}));
    node(&ctx, "b.md", "B", json!({}));
    node(&ctx, "c.md", "C", json!({}));
    node(&ctx, "_stub/x.md", "X", json!({ "_stub": true }));
    edge(&ctx, "a.md", "b.md");
    edge(&ctx, "b.md", "c.md");
    edge(&ctx, "a.md", "_stub/x.md");

    let default = ok(
        &ctx,
        "find_path_between",
        json!({ "from": "a.md", "to": "c.md", "maxDepth": 3 }),
    )
    .await;
    assert_eq!(
        default,
        json!({ "paths": [{
            "nodes": ["a.md", "b.md", "c.md"],
            "edges": [
                { "sourceId": "a.md", "targetId": "b.md", "context": "" },
                { "sourceId": "b.md", "targetId": "c.md", "context": "" },
            ],
            "length": 2,
        }] })
    );

    let with_stubs = ok(
        &ctx,
        "find_path_between",
        json!({ "from": "a.md", "to": "c.md", "maxDepth": 3, "includeStubs": true }),
    )
    .await;
    assert_eq!(with_stubs["paths"].as_array().unwrap().len(), 1);

    let common = ok(
        &ctx,
        "find_path_between",
        json!({ "from": "a.md", "to": "c.md", "includeCommon": true }),
    )
    .await;
    assert_eq!(ids(&common["common"]), ["b.md"]);
}
