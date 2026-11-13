use serde_json::json;

use crate::support::fixture;

#[tokio::test]
async fn semantic_returns_preparing_while_the_embedder_loads() {
    let f = fixture(&[]);
    let out = f
        .ok("search", json!({"query": "test", "mode": "semantic"}))
        .await;
    assert_eq!(out["status"], "preparing");
    let msg = out["message"].as_str().unwrap();
    assert!(msg.contains("still initialising"), "{msg}");
    assert!(msg.to_lowercase().contains("fulltext"), "{msg}");
    assert!(msg.ends_with(
        " You can fall back to search({mode:'fulltext'}) which works without the embedder."
    ));
}

#[tokio::test]
async fn hybrid_and_the_default_mode_return_preparing_too() {
    let f = fixture(&[]);
    for args in [
        json!({"query": "test", "mode": "hybrid"}),
        json!({"query": "test"}),
    ] {
        let out = f.ok("search", args).await;
        assert_eq!(out["status"], "preparing");
        assert!(
            out["message"]
                .as_str()
                .unwrap()
                .contains("still initialising")
        );
    }
}

#[tokio::test]
async fn returns_failed_when_the_embedder_failed_to_load() {
    let f = fixture(&[]);
    f.ctx
        .set_init_error(&anyhow::anyhow!("model checksum mismatch"));
    let out = f
        .ok("search", json!({"query": "test", "mode": "semantic"}))
        .await;
    assert_eq!(out["status"], "failed");
    let msg = out["message"].as_str().unwrap();
    assert!(msg.contains("model checksum mismatch"));
    assert!(msg.to_lowercase().contains("restart the mcp server"));
    assert!(msg.contains("obsidian-brain models check"));
}

#[tokio::test]
async fn the_failed_message_holds_the_error_text() {
    let f = fixture(&[]);
    f.ctx
        .set_init_error(&anyhow::anyhow!("ENOENT: file not found"));
    let out = f
        .ok("search", json!({"query": "test", "mode": "hybrid"}))
        .await;
    assert_eq!(out["status"], "failed");
    assert!(
        out["message"]
            .as_str()
            .unwrap()
            .contains("ENOENT: file not found")
    );
}

#[tokio::test]
async fn fulltext_ignores_the_embedder_state() {
    let f = fixture(&[]);
    let out = f
        .ok("search", json!({"query": "test", "mode": "fulltext"}))
        .await;
    assert!(out.get("status").is_none());
    assert_eq!(out["data"], json!([]));
    assert!(out.get("context").is_some());
}

#[tokio::test]
async fn semantic_runs_once_the_embedder_is_ready() {
    let f = fixture(&[]);
    f.ctx.ensure_embedder_ready().await.unwrap();
    let out = f
        .ok("search", json!({"query": "test", "mode": "semantic"}))
        .await;
    assert!(out.get("status").is_none());
    assert_eq!(out["data"], json!([]));
    assert!(out.get("context").is_some());
}

#[tokio::test]
async fn fulltext_hits_carry_read_hints() {
    let f = fixture(&[("Alpha.md", "resilient components\n")]);
    f.seed_index();
    let out = f
        .ok("search", json!({"query": "resilient", "mode": "fulltext"}))
        .await;
    assert_eq!(out["data"][0]["nodeId"], "Alpha.md");
    assert_eq!(out["context"]["next_actions"][0]["tool"], "read_note");
    assert_eq!(
        out["context"]["state"]["last_search_results"],
        json!(["Alpha.md"])
    );
}

#[tokio::test]
async fn limit_must_be_positive() {
    let f = fixture(&[]);
    assert!(!f.accepts("search", json!({"query": "a", "limit": 0})).await);
    assert!(
        !f.accepts("search", json!({"query": "a", "mode": "fuzzy"}))
            .await
    );
    assert!(
        f.accepts(
            "search",
            json!({"query": "a", "limit": 1, "mode": "fulltext"})
        )
        .await
    );
}
