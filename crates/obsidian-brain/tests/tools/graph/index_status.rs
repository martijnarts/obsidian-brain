use std::sync::Arc;

use obsidian_brain::context::VaultContext;
use obsidian_brain::embeddings::capacity::record_failed_chunk;
use obsidian_brain::pipeline::bootstrap;
use obsidian_brain::store::chunks::{upsert_chunk_row, upsert_chunk_vector};
use obsidian_brain::types::Chunk;
use serde_json::json;

use super::{context, err, node, ok};
use crate::support::FakeEmbedder;

fn setup() -> (tempfile::TempDir, Arc<VaultContext>) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = context(dir.path());
    (dir, ctx)
}

fn sql(ctx: &VaultContext, script: &str) {
    ctx.db.with(|c| c.execute_batch(script)).unwrap();
}

fn chunk(content: &str, hash: &str) -> Chunk {
    Chunk {
        chunk_index: 0,
        heading: None,
        heading_level: None,
        content: content.into(),
        content_hash: hash.into(),
        start_line: 0,
        end_line: 0,
    }
}

fn indexed_note(ctx: &VaultContext, id: &str) {
    node(ctx, id, id.trim_end_matches(".md"), json!({}));
    ctx.db.with(|c| {
        let rowid = upsert_chunk_row(c, id, &chunk("body", &format!("hash-{id}"))).unwrap();
        upsert_chunk_vector(c, rowid, &[0.0; 384]).unwrap();
    });
}

fn no_content_note(ctx: &VaultContext, id: &str) {
    node(ctx, id, id.trim_end_matches(".md"), json!({}));
    ctx.db.with(|c| {
        record_failed_chunk(
            c,
            &format!("{id}#no-content"),
            id,
            "no-embeddable-content",
            None,
        )
        .unwrap()
    });
}

fn failed_note(ctx: &VaultContext, id: &str) {
    node(ctx, id, id.trim_end_matches(".md"), json!({}));
    ctx.db.with(|c| {
        upsert_chunk_row(
            c,
            id,
            &chunk("body too long for embedder", &format!("hash-{id}")),
        )
        .unwrap();
        record_failed_chunk(
            c,
            &format!("{id}::0"),
            id,
            "too-long",
            Some("input length exceeds context length"),
        )
        .unwrap();
    });
}

#[tokio::test]
async fn reports_zero_counts_on_an_empty_index() {
    let (_dir, ctx) = setup();
    let out = ok(&ctx, "index_status", json!({})).await;
    assert_eq!(
        out,
        json!({
            "embeddingModel": "mock/instant",
            "embeddingDim": 384,
            "provider": "mock",
            "notesTotal": 0,
            "notesWithEmbeddings": 0,
            "notesNoEmbeddableContent": 0,
            "notesMissingEmbeddings": 0,
            "summary": "0 / 0 notes indexed",
            "chunksTotal": 0,
            "chunksSkippedInLastRun": 0,
            "failedChunks": [],
            "failedChunksTotal": 0,
            "advertisedMaxTokens": null,
            "discoveredMaxTokens": null,
            "lastReindexReasons": [],
            "reindexInProgress": false,
            "embedderReady": false,
            "initError": null,
        })
    );
    let keys: Vec<&String> = out.as_object().unwrap().keys().collect();
    assert_eq!(keys[0], "embeddingModel");
    assert_eq!(keys[17], "initError");
}

#[tokio::test]
async fn counts_notes_without_stubs_and_chunks() {
    let (_dir, ctx) = setup();
    for id in ["A.md", "B.md", "C.md"] {
        node(&ctx, id, id, json!({}));
    }
    node(&ctx, "_stub/X.md", "X", json!({ "_stub": true }));
    sql(
        &ctx,
        "INSERT INTO chunks (id, node_id, chunk_index, content, content_hash)
         VALUES ('c1', 'A.md', 0, 'alpha chunk', 'h1'), ('c2', 'B.md', 0, 'beta chunk', 'h2')",
    );
    let out = ok(&ctx, "index_status", json!({})).await;
    assert_eq!(out["notesTotal"], 3);
    assert_eq!(out["chunksTotal"], 2);
    assert_eq!(out["notesWithEmbeddings"], 0);
    assert_eq!(out["notesMissingEmbeddings"], 3);
}

#[tokio::test]
async fn surfaces_the_init_error() {
    let (_dir, ctx) = setup();
    ctx.set_init_error(&anyhow::anyhow!("embedder blew up"));
    let out = ok(&ctx, "index_status", json!({})).await;
    assert_eq!(out["initError"], "Error: embedder blew up");
}

#[tokio::test]
async fn reflects_embedder_readiness() {
    let (_dir, ctx) = setup();
    assert_eq!(
        ok(&ctx, "index_status", json!({})).await["embedderReady"],
        false
    );
    ctx.ensure_embedder_ready().await.unwrap();
    assert_eq!(
        ok(&ctx, "index_status", json!({})).await["embedderReady"],
        true
    );
}

#[tokio::test]
async fn tolerates_missing_failed_chunks_and_capability_tables() {
    let (_dir, ctx) = setup();
    sql(
        &ctx,
        "DROP TABLE failed_chunks; DROP TABLE embedder_capability;",
    );
    let out = ok(&ctx, "index_status", json!({})).await;
    assert_eq!(out["failedChunks"], json!([]));
    assert_eq!(out["failedChunksTotal"], 0);
    assert_eq!(out["chunksSkippedInLastRun"], 0);
    assert_eq!(out["notesNoEmbeddableContent"], 0);
    assert_eq!(out["advertisedMaxTokens"], json!(null));
    assert_eq!(out["discoveredMaxTokens"], json!(null));
}

#[tokio::test]
async fn propagates_other_sql_errors() {
    let (_dir, ctx) = setup();
    sql(
        &ctx,
        "DROP TABLE failed_chunks; CREATE TABLE failed_chunks (wrong_col TEXT);",
    );
    let e = err(&ctx, "index_status", json!({})).await;
    assert!(e.contains("no such column"), "{e}");
}

#[tokio::test]
async fn lists_bootstrap_reasons_then_the_manual_reindex_reason() {
    let (_dir, ctx) = setup();
    ctx.db
        .with(|c| bootstrap(c, &FakeEmbedder::stub("old/model", 384, "stub")))
        .unwrap();
    ctx.ensure_embedder_ready().await.unwrap();
    let reasons = ok(&ctx, "index_status", json!({})).await["lastReindexReasons"].clone();
    let reasons: Vec<String> = serde_json::from_value(reasons).unwrap();
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("embedding model changed")),
        "{reasons:?}"
    );

    ctx.set_last_manual_reindex_reason(Some("user-triggered reindex".into()));
    let after = ok(&ctx, "index_status", json!({})).await["lastReindexReasons"].clone();
    let after: Vec<String> = serde_json::from_value(after).unwrap();
    assert_eq!(after.len(), reasons.len() + 1);
    assert_eq!(after.last().unwrap(), "user-triggered reindex");
}

#[tokio::test]
async fn reflects_a_running_reindex() {
    let (_dir, ctx) = setup();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    ctx.enqueue_background_reindex(async move {
        let _ = rx.await;
        Ok(())
    });
    while !ctx.reindex_in_progress() {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        ok(&ctx, "index_status", json!({})).await["reindexInProgress"],
        true
    );
    tx.send(()).unwrap();
    ctx.wait_for_background_reindex().await;
    assert_eq!(
        ok(&ctx, "index_status", json!({})).await["reindexInProgress"],
        false
    );
}

#[tokio::test]
async fn splits_missing_notes_into_three_buckets() {
    let (_dir, ctx) = setup();
    for i in 0..3 {
        indexed_note(&ctx, &format!("indexed-{i}.md"));
    }
    for i in 0..2 {
        no_content_note(&ctx, &format!("empty-{i}.md"));
    }
    failed_note(&ctx, "failed-0.md");
    let out = ok(&ctx, "index_status", json!({})).await;
    assert_eq!(out["notesTotal"], 6);
    assert_eq!(out["notesWithEmbeddings"], 3);
    assert_eq!(out["notesNoEmbeddableContent"], 2);
    assert_eq!(out["notesMissingEmbeddings"], 1);
    assert_eq!(out["failedChunksTotal"], 3);
    assert!(out["failedChunks"][0]["reason"].is_string());
    assert!(out["failedChunks"][0]["at"].is_i64());
}

#[tokio::test]
async fn the_summary_names_each_non_zero_bucket() {
    let (_dir, ctx) = setup();
    indexed_note(&ctx, "a.md");
    no_content_note(&ctx, "b.md");
    failed_note(&ctx, "c.md");
    let out = ok(&ctx, "index_status", json!({})).await;
    assert_eq!(
        out["summary"],
        "1 / 3 notes indexed; 1 have no embeddable content (empty / frontmatter-only); 1 failed to embed"
    );
}

#[tokio::test]
async fn all_indexed_leaves_the_other_buckets_empty() {
    let (_dir, ctx) = setup();
    for i in 0..5 {
        indexed_note(&ctx, &format!("n-{i}.md"));
    }
    let out = ok(&ctx, "index_status", json!({})).await;
    assert_eq!(out["notesTotal"], 5);
    assert_eq!(out["notesWithEmbeddings"], 5);
    assert_eq!(out["notesNoEmbeddableContent"], 0);
    assert_eq!(out["notesMissingEmbeddings"], 0);
}

#[tokio::test]
async fn content_less_notes_are_not_missing() {
    let (_dir, ctx) = setup();
    for i in 0..100 {
        no_content_note(&ctx, &format!("stub-{i}.md"));
    }
    let out = ok(&ctx, "index_status", json!({})).await;
    assert_eq!(out["notesTotal"], 100);
    assert_eq!(out["notesWithEmbeddings"], 0);
    assert_eq!(out["notesNoEmbeddableContent"], 100);
    assert_eq!(out["notesMissingEmbeddings"], 0);
}

#[tokio::test]
async fn reads_capacity_for_the_live_model() {
    let (_dir, ctx) = setup();
    sql(
        &ctx,
        "INSERT INTO embedder_capability (embedder_id, model_hash, advertised_max_tokens, discovered_max_tokens, discovered_at, method)
         VALUES ('mock/instant', 'h', 512, 256, 0, 'probe')",
    );
    let out = ok(&ctx, "index_status", json!({})).await;
    assert_eq!(out["advertisedMaxTokens"], 512);
    assert_eq!(out["discoveredMaxTokens"], 256);
}
