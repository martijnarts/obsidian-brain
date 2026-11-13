use serde_json::json;

use super::{context, ok};
use crate::support::{copy_dir, fixture_vault};

#[tokio::test]
async fn indexes_the_vault_and_records_the_manual_reason() {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("vault");
    copy_dir(&fixture_vault(), &vault);
    let ctx = context(&vault);

    let first = ok(&ctx, "reindex", json!({})).await;
    assert!(first["nodesIndexed"].as_u64().unwrap() > 0, "{first}");
    assert!(first["stubsPruned"].is_u64());
    assert_eq!(
        ctx.last_manual_reindex_reason().as_deref(),
        Some("user-triggered reindex")
    );
    let keys: Vec<&String> = first.as_object().unwrap().keys().collect();
    assert_eq!(keys.first().map(|k| k.as_str()), Some("nodesIndexed"));
    assert_eq!(keys.last().map(|k| k.as_str()), Some("stubsPruned"));

    let second = ok(&ctx, "reindex", json!({})).await;
    assert_eq!(second["nodesIndexed"], 0);

    let status = ok(&ctx, "index_status", json!({})).await;
    assert_eq!(
        status["lastReindexReasons"].as_array().unwrap().last(),
        Some(&json!("user-triggered reindex"))
    );
}
