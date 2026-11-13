//! Tests of the embedder capacity and its drift floor. The Ollama probe runs against a local mock server.

use std::collections::HashMap;

use obsidian_brain::embeddings::Embedder;
use obsidian_brain::embeddings::capacity::{
    CapacityMethod, MIN_DISCOVERED_TOKENS, get_capacity_with_env, record_failed_chunk,
    reduce_discovered_max_tokens, reset_discovered_capacity,
};
use obsidian_brain::embeddings::metadata_cache::model_hash;
use rusqlite::Connection;
use serde_json::json;

use crate::mock_http::{MockServer, json};
use crate::support::{StubEmbedder, env, open_test_db};

fn no_env() -> HashMap<String, String> {
    HashMap::new()
}

fn discovered(conn: &Connection, e: &dyn Embedder) -> Option<i64> {
    conn.query_row(
        "SELECT discovered_max_tokens FROM embedder_capability WHERE embedder_id = ?",
        [e.model_identifier()],
        |r| r.get(0),
    )
    .ok()
}

#[tokio::test]
async fn reads_model_max_length_from_the_tokenizer() {
    let conn = open_test_db();
    let emb = StubEmbedder::transformers("Xenova/bge-large-en-v1.5", Some(8192.0));
    let cap = get_capacity_with_env(&conn, &emb, &no_env()).await.unwrap();
    assert_eq!(cap.advertised_max_tokens, 8192);
    assert_eq!(cap.discovered_max_tokens, 8192);
    assert_eq!(cap.method, CapacityMethod::TokenizerConfig);
    assert_eq!(cap.chunk_budget_tokens, 7372);
    assert_eq!(cap.chunk_budget_chars, 18430);
}

#[tokio::test]
async fn uses_the_tokenizer_value_verbatim() {
    let conn = open_test_db();
    let emb = StubEmbedder::transformers("Xenova/multilingual-e5-small", Some(512.0));
    let cap = get_capacity_with_env(&conn, &emb, &no_env()).await.unwrap();
    assert_eq!(cap.advertised_max_tokens, 512);
    assert_eq!(cap.method, CapacityMethod::TokenizerConfig);
    assert_eq!(cap.chunk_budget_tokens, 460);
}

#[tokio::test]
async fn falls_back_to_512_without_a_tokenizer_limit() {
    let conn = open_test_db();
    for limit in [None, Some(f64::INFINITY), Some(1e30), Some(0.0)] {
        let emb = StubEmbedder::transformers(&format!("custom/unknown-{limit:?}"), limit);
        let cap = get_capacity_with_env(&conn, &emb, &no_env()).await.unwrap();
        assert_eq!(cap.advertised_max_tokens, 512);
        assert_eq!(cap.method, CapacityMethod::Fallback);
    }
}

#[tokio::test]
async fn ollama_context_length_from_api_show() {
    let conn = open_test_db();
    let server = MockServer::start(vec![json(
        200,
        json!({"model_info": {"llama.context_length": 8192}}),
    )]);
    let emb = StubEmbedder::ollama("llama3-embed", &server.url);
    let cap = get_capacity_with_env(&conn, &emb, &no_env()).await.unwrap();
    assert_eq!(cap.advertised_max_tokens, 8192);
    assert_eq!(cap.discovered_max_tokens, 8192);
    assert_eq!(cap.method, CapacityMethod::ApiShow);
    assert_eq!(cap.chunk_budget_tokens, 7372);
    let req = &server.requests()[0];
    assert_eq!(
        (req.method.as_str(), req.path.as_str()),
        ("POST", "/api/show")
    );
    assert_eq!(req.json(), json!({"name": "llama3-embed"}));
}

#[tokio::test]
async fn ollama_context_length_by_key_suffix() {
    let conn = open_test_db();
    let server = MockServer::start(vec![json(
        200,
        json!({"model_info": {"nomic.context_length": 8192, "other.field": "ignored"}}),
    )]);
    let emb = StubEmbedder::ollama("nomic-embed-text", &server.url);
    let cap = get_capacity_with_env(&conn, &emb, &no_env()).await.unwrap();
    assert_eq!(cap.advertised_max_tokens, 8192);
    assert_eq!(cap.method, CapacityMethod::ApiShow);
}

#[tokio::test]
async fn ollama_num_ctx_from_parameters() {
    let conn = open_test_db();
    let server = MockServer::start(vec![json(
        200,
        json!({"model_info": {}, "parameters": "num_ctx 4096\nstop \"<|eot_id|>\""}),
    )]);
    let emb = StubEmbedder::ollama("custom-embed", &server.url);
    let cap = get_capacity_with_env(&conn, &emb, &no_env()).await.unwrap();
    assert_eq!(cap.advertised_max_tokens, 4096);
    assert_eq!(cap.method, CapacityMethod::ApiShow);
}

#[tokio::test]
async fn ollama_falls_back_to_512_when_api_show_fails() {
    let conn = open_test_db();
    let server = MockServer::start(vec![json(404, json!({}))]);
    let emb = StubEmbedder::ollama("missing-model", &server.url);
    let cap = get_capacity_with_env(&conn, &emb, &no_env()).await.unwrap();
    assert_eq!(cap.advertised_max_tokens, 512);
    assert_eq!(cap.method, CapacityMethod::Fallback);
}

#[tokio::test]
async fn caches_the_probe_result() {
    let conn = open_test_db();
    let server = MockServer::start(vec![
        json(200, json!({"model_info": {"llama.context_length": 4096}})),
        json(200, json!({"model_info": {"llama.context_length": 4096}})),
    ]);
    let emb = StubEmbedder::ollama("cached-model", &server.url);
    let first = get_capacity_with_env(&conn, &emb, &no_env()).await.unwrap();
    assert_eq!(first.advertised_max_tokens, 4096);
    assert_eq!(server.requests().len(), 1);
    let second = get_capacity_with_env(&conn, &emb, &no_env()).await.unwrap();
    assert_eq!(second.advertised_max_tokens, 4096);
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn second_transformers_call_reads_the_cache() {
    let conn = open_test_db();
    let emb = StubEmbedder::transformers("Xenova/bge-large-en-v1.5", Some(8192.0));
    get_capacity_with_env(&conn, &emb, &no_env()).await.unwrap();
    let again = StubEmbedder::transformers("Xenova/bge-large-en-v1.5", None);
    let second = get_capacity_with_env(&conn, &again, &no_env())
        .await
        .unwrap();
    assert_eq!(second.advertised_max_tokens, 8192);
    assert_eq!(second.method, CapacityMethod::TokenizerConfig);
}

#[tokio::test]
async fn env_override_beats_the_tokenizer() {
    let conn = open_test_db();
    let emb = StubEmbedder::transformers("Xenova/bge-large-en-v1.5", Some(8192.0));
    let cap = get_capacity_with_env(
        &conn,
        &emb,
        &env(&[("OBSIDIAN_BRAIN_MAX_CHUNK_TOKENS", "2000")]),
    )
    .await
    .unwrap();
    assert_eq!(cap.advertised_max_tokens, 2000);
    assert_eq!(cap.discovered_max_tokens, 2000);
    assert_eq!(cap.method, CapacityMethod::Manual);
    assert_eq!(cap.chunk_budget_tokens, 1800);
}

#[tokio::test]
async fn env_override_skips_the_ollama_probe() {
    let conn = open_test_db();
    let server = MockServer::start(vec![json(
        200,
        json!({"model_info": {"llama.context_length": 8192}}),
    )]);
    let emb = StubEmbedder::ollama("big-model", &server.url);
    let cap = get_capacity_with_env(
        &conn,
        &emb,
        &env(&[("OBSIDIAN_BRAIN_MAX_CHUNK_TOKENS", "1024")]),
    )
    .await
    .unwrap();
    assert_eq!(cap.advertised_max_tokens, 1024);
    assert_eq!(cap.method, CapacityMethod::Manual);
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn per_model_env_override_beats_the_global_one() {
    let conn = open_test_db();
    let emb = StubEmbedder::transformers("Xenova/bge-large-en-v1.5", Some(8192.0));
    let key = format!(
        "OBSIDIAN_BRAIN_MAX_CHUNK_TOKENS_{}",
        model_hash("Xenova/bge-large-en-v1.5")
    );
    let e = env(&[
        ("OBSIDIAN_BRAIN_MAX_CHUNK_TOKENS", "1000"),
        (key.as_str(), "3000"),
    ]);
    let cap = get_capacity_with_env(&conn, &emb, &e).await.unwrap();
    assert_eq!(cap.advertised_max_tokens, 3000);
    assert_eq!(cap.method, CapacityMethod::Manual);
}

#[test]
fn records_a_failed_chunk() {
    let conn = open_test_db();
    record_failed_chunk(
        &conn,
        "note.md::0",
        "note.md",
        "too_large",
        Some("Token limit exceeded"),
    )
    .unwrap();
    let (note, reason, msg): (String, String, String) = conn
        .query_row(
            "SELECT note_id, reason, error_message FROM failed_chunks WHERE chunk_id = 'note.md::0'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (note.as_str(), reason.as_str(), msg.as_str()),
        ("note.md", "too_large", "Token limit exceeded")
    );
}

#[test]
fn failed_chunk_upserts() {
    let conn = open_test_db();
    record_failed_chunk(
        &conn,
        "note.md::0",
        "note.md",
        "too_large",
        Some("first error"),
    )
    .unwrap();
    record_failed_chunk(
        &conn,
        "note.md::0",
        "note.md",
        "embed_error",
        Some("second error"),
    )
    .unwrap();
    let (n, reason): (i64, String) = conn
        .query_row(
            "SELECT COUNT(*), MAX(reason) FROM failed_chunks WHERE chunk_id = 'note.md::0'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((n, reason.as_str()), (1, "embed_error"));
}

#[tokio::test]
async fn reduce_halves_the_failing_count() {
    let conn = open_test_db();
    let emb = StubEmbedder::transformers("Xenova/bge-large-en-v1.5", Some(8192.0));
    get_capacity_with_env(&conn, &emb, &no_env()).await.unwrap();
    reduce_discovered_max_tokens(&conn, &emb, 600).unwrap();
    assert_eq!(discovered(&conn, &emb), Some(300));
}

#[tokio::test]
async fn reduce_is_a_one_way_ratchet() {
    let conn = open_test_db();
    let emb = StubEmbedder::transformers("Xenova/bge-large-en-v1.5", Some(8192.0));
    get_capacity_with_env(&conn, &emb, &no_env()).await.unwrap();
    reduce_discovered_max_tokens(&conn, &emb, 200).unwrap();
    reduce_discovered_max_tokens(&conn, &emb, 1000).unwrap();
    assert_eq!(discovered(&conn, &emb), Some(256));
}

#[tokio::test]
async fn reduce_never_goes_below_256() {
    let conn = open_test_db();
    let emb = StubEmbedder::transformers("Xenova/bge-large-en-v1.5", Some(8192.0));
    get_capacity_with_env(&conn, &emb, &no_env()).await.unwrap();
    reduce_discovered_max_tokens(&conn, &emb, 100).unwrap();
    assert_eq!(discovered(&conn, &emb), Some(256));
}

#[tokio::test]
async fn reduce_floor_is_the_advertised_limit_of_a_tiny_model() {
    let conn = open_test_db();
    let emb = StubEmbedder::transformers("test/model", Some(128.0));
    get_capacity_with_env(&conn, &emb, &no_env()).await.unwrap();
    reduce_discovered_max_tokens(&conn, &emb, 1).unwrap();
    assert_eq!(discovered(&conn, &emb), Some(128));
}

#[test]
fn reduce_inserts_a_fresh_row() {
    let conn = open_test_db();
    let emb = StubEmbedder::transformers("Xenova/bge-small-en-v1.5", Some(512.0));
    reduce_discovered_max_tokens(&conn, &emb, 400).unwrap();
    assert_eq!(discovered(&conn, &emb), Some(256));
}

#[test]
fn min_discovered_tokens_is_256() {
    assert_eq!(MIN_DISCOVERED_TOKENS, 256);
}

#[tokio::test]
async fn floor_stops_runaway_drift() {
    let conn = open_test_db();
    let emb = StubEmbedder::transformers("Xenova/multilingual-e5-base", Some(512.0));
    get_capacity_with_env(&conn, &emb, &no_env()).await.unwrap();
    for _ in 0..10 {
        reduce_discovered_max_tokens(&conn, &emb, 200).unwrap();
    }
    assert_eq!(discovered(&conn, &emb), Some(256));
}

#[tokio::test]
async fn floor_adapts_to_small_models() {
    let conn = open_test_db();
    let emb = StubEmbedder::transformers("test/tiny", Some(128.0));
    get_capacity_with_env(&conn, &emb, &no_env()).await.unwrap();
    reduce_discovered_max_tokens(&conn, &emb, 50).unwrap();
    assert_eq!(discovered(&conn, &emb), Some(128));
}

#[tokio::test]
async fn reset_wipes_drift() {
    let conn = open_test_db();
    let emb = StubEmbedder::transformers("Xenova/bge-large-en-v1.5", Some(8192.0));
    get_capacity_with_env(&conn, &emb, &no_env()).await.unwrap();
    reduce_discovered_max_tokens(&conn, &emb, 2000).unwrap();
    assert_eq!(discovered(&conn, &emb), Some(1000));
    reset_discovered_capacity(&conn, &emb).unwrap();
    assert_eq!(discovered(&conn, &emb), Some(8192));
}

#[test]
fn reset_without_a_row_is_a_no_op() {
    let conn = open_test_db();
    let emb = StubEmbedder::new("test/uninit", 384);
    reset_discovered_capacity(&conn, &emb).unwrap();
    assert_eq!(discovered(&conn, &emb), None);
}
