//! Tests of the Ollama embedder and its `num_ctx`, against a local mock
//! server.

use std::collections::HashMap;

use obsidian_brain::embeddings::ollama::{
    OllamaEmbedder, OllamaPhase, is_ollama_official_namespace, ollama_namespace_of,
};
use obsidian_brain::embeddings::{Embedder, EmbedderMetadata, PrefixSource, TaskType};
use serde_json::json;

use crate::mock_http::{MockServer, Reply, json, ndjson, text};

fn ok(embedding: Vec<f64>) -> Reply {
    json(200, json!({ "embedding": embedding }))
}

fn embedder(server: &MockServer, model: &str) -> OllamaEmbedder {
    OllamaEmbedder::new(Some(&server.url), Some(model), None, None, None).with_env(HashMap::new())
}

async fn prompt_for(model: &str, input: &str, task: TaskType) -> String {
    let server = MockServer::start(vec![ok(vec![0.1, 0.2, 0.3])]);
    embedder(&server, model).embed(input, task).await.unwrap();
    let req = &server.requests()[0];
    assert_eq!(req.path, "/api/embeddings");
    let body = req.json();
    assert_eq!(body["model"], model);
    body["prompt"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn family_prefixes() {
    use TaskType::{Document, Query};
    let cases = [
        (
            "nomic-embed-text",
            Document,
            "search_document: the quick brown fox",
        ),
        (
            "nomic-embed-text",
            Query,
            "search_query: the quick brown fox",
        ),
        ("qwen3-embedding:0.6b", Query, "Query: the quick brown fox"),
        ("qwen3-embedding:0.6b", Document, "the quick brown fox"),
        ("multilingual-e5-small", Query, "query: the quick brown fox"),
        (
            "multilingual-e5-small",
            Document,
            "passage: the quick brown fox",
        ),
        ("multilingual-e5-large", Query, "query: the quick brown fox"),
        (
            "multilingual-e5-large",
            Document,
            "passage: the quick brown fox",
        ),
        ("bge-m3", Query, "the quick brown fox"),
        ("bge-m3", Document, "the quick brown fox"),
        (
            "mxbai-embed-large",
            Query,
            "Represent this sentence for searching relevant passages: the quick brown fox",
        ),
        ("mxbai-embed-large", Document, "the quick brown fox"),
        (
            "mixedbread-ai/some-model",
            Query,
            "Represent this sentence for searching relevant passages: the quick brown fox",
        ),
        ("some-unknown-model", Query, "the quick brown fox"),
    ];
    for (model, task, want) in cases {
        assert_eq!(
            prompt_for(model, "the quick brown fox", task).await,
            want,
            "{model} {task:?}"
        );
    }
}

#[tokio::test]
async fn dimensions_before_and_after_embed() {
    let server = MockServer::start(vec![ok(vec![0.001; 768])]);
    let e = embedder(&server, "nomic-embed-text");
    let err = e.dimensions().unwrap_err().to_string();
    assert!(err.contains("dimensions not known yet"));
    assert!(!err.contains("OLLAMA_EMBEDDING_DIM"));
    let v = e.embed("anything", TaskType::Document).await.unwrap();
    assert_eq!(v.len(), 768);
    assert_eq!(e.dimensions().unwrap(), 768);
}

#[test]
fn declared_dim_is_known_at_once() {
    let e = OllamaEmbedder::new(None, Some("nomic-embed-text"), Some(768), None, None);
    assert_eq!(e.dimensions().unwrap(), 768);
}

#[tokio::test]
async fn dim_mismatch_errors() {
    let server = MockServer::start(vec![ok(vec![0.0; 768]), ok(vec![0.0; 1024])]);
    let e = embedder(&server, "nomic-embed-text");
    e.embed("first", TaskType::Document).await.unwrap();
    let err = e.embed("second", TaskType::Document).await.unwrap_err();
    assert!(err.to_string().contains("Ollama dim mismatch"));
}

#[tokio::test]
async fn http_error_names_the_fix() {
    let server = MockServer::start(vec![text(404, "model not found")]);
    let e = embedder(&server, "nomic-embed-text");
    let err = e
        .embed("x", TaskType::Document)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.starts_with("Ollama embed failed: HTTP 404 Not Found — model not found."),
        "{err}"
    );
    assert!(err.contains("ollama pull nomic-embed-text"));
}

#[tokio::test]
async fn empty_embedding_is_an_error() {
    let server = MockServer::start(vec![ok(vec![])]);
    let e = embedder(&server, "some-chat-model");
    let err = e
        .embed("x", TaskType::Document)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("empty vector"));
}

#[tokio::test]
async fn dimensions_returns_the_last_embed_error() {
    let server = MockServer::start(vec![text(404, "model not found")]);
    let e = embedder(&server, "qwen3-embedding:0.6b");
    assert!(e.embed("x", TaskType::Document).await.is_err());
    let err = e.dimensions().unwrap_err().to_string();
    assert!(err.contains("HTTP 404") && err.contains("ollama pull qwen3-embedding:0.6b"));
    assert!(!err.contains("dimensions not known yet"));
}

#[tokio::test]
async fn dimensions_returns_the_last_init_error() {
    let server = MockServer::start(vec![
        json(404, json!({})),
        json(404, json!({})),
        text(404, "model not found"),
    ]);
    let e = embedder(&server, "qwen3-embedding:0.6b").with_env(HashMap::from([(
        "OBSIDIAN_BRAIN_OLLAMA_AUTO_PULL".into(),
        "0".into(),
    )]));
    let err = e.init().await.unwrap_err().to_string();
    assert!(err.contains("HTTP 404"));
    let err = e.dimensions().unwrap_err().to_string();
    assert!(err.contains("HTTP 404") && err.contains("ollama pull qwen3-embedding:0.6b"));
}

#[tokio::test]
async fn last_error_clears_on_success() {
    let server = MockServer::start(vec![text(404, "first attempt"), ok(vec![0.001; 384])]);
    let e = embedder(&server, "nomic-embed-text");
    assert!(e.embed("first", TaskType::Document).await.is_err());
    assert!(e.dimensions().unwrap_err().to_string().contains("HTTP 404"));
    e.embed("second", TaskType::Document).await.unwrap();
    assert_eq!(e.dimensions().unwrap(), 384);
}

#[test]
fn identifier_and_provider() {
    let e = OllamaEmbedder::new(None, Some("nomic-embed-text"), None, None, None);
    assert_eq!(e.model_identifier(), "ollama:nomic-embed-text");
    assert_eq!(e.provider_name(), "ollama");
}

fn meta(model: &str, source: PrefixSource) -> EmbedderMetadata {
    EmbedderMetadata {
        model_id: format!("ollama:{model}"),
        dim: Some(1024),
        max_tokens: 512,
        query_prefix: String::new(),
        document_prefix: String::new(),
        prefix_source: source,
        base_model: None,
        size_bytes: None,
    }
}

#[tokio::test]
async fn fallback_metadata_uses_the_family_heuristic() {
    let server = MockServer::start(vec![ok(vec![0.001; 1024])]);
    let e = embedder(&server, "qwen3-embedding:0.6b");
    e.set_metadata(meta("qwen3-embedding:0.6b", PrefixSource::Fallback))
        .unwrap();
    e.embed("butter chicken", TaskType::Query).await.unwrap();
    assert_eq!(
        server.requests()[0].json()["prompt"],
        "Query: butter chicken"
    );
}

#[tokio::test]
async fn authoritative_empty_prefix_wins() {
    for (model, source) in [
        ("bge-m3", PrefixSource::Seed),
        ("qwen3-embedding:0.6b", PrefixSource::Override),
    ] {
        let server = MockServer::start(vec![ok(vec![0.001; 384])]);
        let e = embedder(&server, model);
        e.set_metadata(meta(model, source)).unwrap();
        e.embed("test", TaskType::Query).await.unwrap();
        assert_eq!(server.requests()[0].json()["prompt"], "test");
    }
}

#[tokio::test]
async fn text_placeholders_are_substituted() {
    let server = MockServer::start(vec![ok(vec![0.1])]);
    let e = embedder(&server, "x");
    e.set_metadata(EmbedderMetadata {
        query_prefix: "Task: {text}\nQuery: {text}".into(),
        ..meta("x", PrefixSource::Seed)
    })
    .unwrap();
    e.embed("hi", TaskType::Query).await.unwrap();
    assert_eq!(server.requests()[0].json()["prompt"], "Task: hi\nQuery: hi");
}

fn show(arch: &str, dim: u64, ctx: u64) -> Reply {
    json(
        200,
        json!({
            "capabilities": ["embedding"],
            "model_info": {
                format!("{arch}.embedding_length"): dim,
                format!("{arch}.context_length"): ctx,
            },
        }),
    )
}

fn tags(name: &str, digest: &str) -> Reply {
    json(
        200,
        json!({ "models": [{ "name": name, "digest": digest }] }),
    )
}

fn tags_empty() -> Reply {
    json(200, json!({ "models": [] }))
}

#[tokio::test]
async fn init_reads_show_and_tags_without_an_embed() {
    let server = MockServer::start(vec![
        show("nomic-bert", 768, 2048),
        tags("nomic-embed-text:latest", "sha256:abc"),
    ]);
    let e = embedder(&server, "nomic-embed-text");
    e.init().await.unwrap();
    assert_eq!(server.paths(), vec!["/api/show", "/api/tags"]);
    assert_eq!(
        server.requests()[0].json(),
        json!({"model": "nomic-embed-text"})
    );
    assert_eq!(e.dimensions().unwrap(), 768);
    assert_eq!(e.identity_hash().as_deref(), Some("sha256:abc"));
    assert_eq!(e.context_length(), Some(2048));
}

#[tokio::test]
async fn init_probes_with_an_embed_without_embedding_length() {
    let server = MockServer::start(vec![
        json(200, json!({"capabilities": ["embedding"]})),
        tags("foo:latest", "sha256:fb"),
        ok(vec![0.0; 7]),
    ]);
    let e = embedder(&server, "foo");
    e.init().await.unwrap();
    assert_eq!(e.dimensions().unwrap(), 7);
    assert_eq!(e.context_length(), None);
}

#[tokio::test]
async fn init_rejects_a_model_without_the_embedding_capability() {
    let server = MockServer::start(vec![json(
        200,
        json!({"capabilities": ["completion", "tools"]}),
    )]);
    let e = embedder(&server, "some-llm");
    let err = e.init().await.unwrap_err().to_string();
    assert!(err.contains("not \"embedding\""), "{err}");
}

#[tokio::test]
async fn declared_dim_skips_the_probe_embed() {
    let server = MockServer::start(vec![
        show("nomic-bert", 768, 2048),
        tags("nomic-embed-text:latest", "sha256:def"),
    ]);
    let e = OllamaEmbedder::new(
        Some(&server.url),
        Some("nomic-embed-text"),
        Some(768),
        None,
        None,
    );
    e.init().await.unwrap();
    assert_eq!(server.requests().len(), 2);
    assert_eq!(e.dimensions().unwrap(), 768);
    assert_eq!(e.identity_hash().as_deref(), Some("sha256:def"));
}

#[tokio::test]
async fn identity_hash_is_none_when_tags_fail() {
    let server = MockServer::start(vec![show("nomic-bert", 768, 2048), Reply::Drop]);
    let e = embedder(&server, "nomic-embed-text");
    e.init().await.unwrap();
    assert_eq!(e.dimensions().unwrap(), 768);
    assert_eq!(e.identity_hash(), None);
}

#[tokio::test]
async fn identity_hash_is_none_for_a_missing_model() {
    let server = MockServer::start(vec![
        show("nomic-bert", 768, 2048),
        tags("some-other-model:latest", "sha256:xyz"),
    ]);
    let e = embedder(&server, "nomic-embed-text");
    e.init().await.unwrap();
    assert_eq!(e.identity_hash(), None);
}

#[tokio::test]
async fn identity_hash_matches_name_latest() {
    let server = MockServer::start(vec![
        show("bert", 1024, 8194),
        tags("bge-m3:latest", "sha256:567ca40"),
    ]);
    let e = embedder(&server, "bge-m3");
    e.init().await.unwrap();
    assert_eq!(e.identity_hash().as_deref(), Some("sha256:567ca40"));
}

fn show_404() -> Reply {
    json(
        404,
        json!({"error": "model \"test\" not found, try pulling it first"}),
    )
}

#[tokio::test]
async fn missing_model_is_pulled_then_probed() {
    let server = MockServer::start(vec![
        show_404(),
        tags_empty(),
        ndjson(&[
            r#"{"status":"pulling manifest"}"#,
            r#"{"status":"downloading","digest":"sha256:abc","total":1000000,"completed":500000}"#,
            r#"{"status":"downloading","digest":"sha256:abc","total":1000000,"completed":1000000}"#,
            r#"{"status":"success"}"#,
        ]),
        show("qwen3", 1024, 32768),
        tags("qwen3-embedding:0.6b", "sha256:def"),
    ]);
    let e = embedder(&server, "qwen3-embedding:0.6b");
    e.init().await.unwrap();
    assert_eq!(e.dimensions().unwrap(), 1024);
    assert_eq!(e.context_length(), Some(32768));
    assert_eq!(e.identity_hash().as_deref(), Some("sha256:def"));
    let pull = server
        .requests()
        .into_iter()
        .find(|r| r.path == "/api/pull")
        .unwrap();
    assert_eq!(
        pull.json(),
        json!({"model": "qwen3-embedding:0.6b", "stream": true})
    );
    assert_eq!(e.phase(), OllamaPhase::Ready);
}

#[tokio::test]
async fn pull_error_line_fails_init() {
    let server = MockServer::start(vec![
        show_404(),
        tags_empty(),
        ndjson(&[
            r#"{"status":"pulling manifest"}"#,
            r#"{"error":"file does not exist"}"#,
        ]),
    ]);
    let e = embedder(&server, "bogus-model");
    let err = e.init().await.unwrap_err().to_string();
    assert!(
        err.starts_with("Ollama auto-pull failed: file does not exist."),
        "{err}"
    );
    assert!(err.contains("OBSIDIAN_BRAIN_OLLAMA_AUTO_PULL=0"));
    assert!(
        e.dimensions()
            .unwrap_err()
            .to_string()
            .contains("Ollama auto-pull failed")
    );
}

#[tokio::test]
async fn auto_pull_off_falls_through_to_the_embed_error() {
    let server = MockServer::start(vec![show_404(), tags_empty(), text(404, "model not found")]);
    let e = embedder(&server, "qwen3-embedding:0.6b").with_env(HashMap::from([(
        "OBSIDIAN_BRAIN_OLLAMA_AUTO_PULL".into(),
        "0".into(),
    )]));
    let err = e.init().await.unwrap_err().to_string();
    assert!(err.contains("HTTP 404") && err.contains("ollama pull qwen3-embedding:0.6b"));
    assert!(!server.called("/api/pull"));
}

#[test]
fn phase_starts_not_started() {
    let e = OllamaEmbedder::new(None, Some("nomic-embed-text"), None, None, None);
    assert_eq!(e.phase(), OllamaPhase::NotStarted);
    assert_eq!(
        serde_json::to_value(e.phase()).unwrap(),
        json!({"status": "not-started"})
    );
}

#[tokio::test]
async fn phase_is_ready_after_init() {
    let server = MockServer::start(vec![
        json(200, json!({"model_info": {"general.architecture": "bert"}})),
        json(
            200,
            json!({"models": [{"name": "nomic-embed-text", "digest": "abc"}]}),
        ),
        ok(vec![0.1, 0.2]),
    ]);
    let e = embedder(&server, "nomic-embed-text");
    e.init().await.unwrap();
    assert_eq!(e.phase(), OllamaPhase::Ready);
    assert_eq!(e.identity_hash().as_deref(), Some("abc"));
}

#[tokio::test]
async fn phase_is_failed_after_a_failed_init() {
    let server = MockServer::start(vec![text(500, "boom")]);
    let e = embedder(&server, "nomic-embed-text").with_env(HashMap::from([(
        "OBSIDIAN_BRAIN_OLLAMA_AUTO_PULL".into(),
        "0".into(),
    )]));
    assert!(e.init().await.is_err());
    assert!(matches!(e.phase(), OllamaPhase::Failed { .. }));
}

#[test]
fn pulling_phase_serialises_like_the_typescript_one() {
    let p = OllamaPhase::Pulling {
        completed_mb: 1,
        total_mb: 2,
        pct: 50,
        phase: "downloading".into(),
    };
    assert_eq!(
        serde_json::to_value(p).unwrap(),
        json!({"status": "pulling", "completedMb": 1, "totalMb": 2, "pct": 50, "phase": "downloading"})
    );
}

// --- BYOM auto-pull gate ---------------------------------------------------

fn pull_success() -> Reply {
    ndjson(&[r#"{"status":"success"}"#])
}

fn show_ok() -> Reply {
    json(
        200,
        json!({"model_info": {"general.architecture": "bert", "bert.embedding_length": 384, "bert.context_length": 512}}),
    )
}

fn gated(
    server: &MockServer,
    model: &str,
    preset: Option<&str>,
    env: &[(&str, &str)],
) -> OllamaEmbedder {
    OllamaEmbedder::new(Some(&server.url), Some(model), None, None, preset).with_env(
        env.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
}

async fn pulls(model: &str, preset: Option<&str>, env: &[(&str, &str)]) -> bool {
    let server = MockServer::start(vec![
        show_404(),
        tags_empty(),
        pull_success(),
        show_ok(),
        tags(model, "sha256:abc"),
    ]);
    let e = gated(&server, model, preset, env);
    e.init().await.unwrap();
    assert_eq!(e.phase(), OllamaPhase::Ready);
    server.called("/api/pull")
}

#[tokio::test]
async fn gate_preset_model_pulls() {
    assert!(pulls("qwen3-embedding:0.6b", Some("multilingual-ollama"), &[]).await);
}

#[tokio::test]
async fn gate_bare_name_pulls() {
    assert!(pulls("nomic-embed-text", None, &[]).await);
}

#[tokio::test]
async fn gate_library_prefix_pulls() {
    assert!(pulls("library/llama3:8b", None, &[]).await);
}

#[tokio::test]
async fn gate_third_party_with_opt_in_pulls() {
    assert!(
        pulls(
            "user/custom-fork",
            None,
            &[("OBSIDIAN_BRAIN_OLLAMA_BYOM_AUTO_PULL", "1")]
        )
        .await
    );
}

#[tokio::test]
async fn gate_blocks_a_third_party_namespace() {
    let server = MockServer::start(vec![show_404(), tags_empty()]);
    let e = gated(&server, "user/custom-fork", None, &[]);
    let err = e.init().await.unwrap_err().to_string();
    assert!(err.contains("not pulled and auto-pull was skipped"));
    assert!(err.contains("third-party namespace 'user'"));
    assert!(err.contains("ollama pull user/custom-fork"));
    assert!(err.contains("OBSIDIAN_BRAIN_OLLAMA_BYOM_AUTO_PULL=1"));
    assert!(err.contains("EMBEDDING_PRESET=multilingual-ollama"));
    assert!(!server.called("/api/pull"));
    match e.phase() {
        OllamaPhase::Failed { error } => assert!(error.contains("ollama pull user/custom-fork")),
        other => panic!("phase {other:?}"),
    }
}

#[tokio::test]
async fn gate_master_switch_wins_over_a_preset() {
    let server = MockServer::start(vec![show_404(), tags_empty(), text(404, "model not found")]);
    let e = gated(
        &server,
        "qwen3-embedding:0.6b",
        Some("multilingual-ollama"),
        &[("OBSIDIAN_BRAIN_OLLAMA_AUTO_PULL", "0")],
    );
    let err = e.init().await.unwrap_err().to_string();
    assert!(err.contains("HTTP 404") && err.contains("ollama pull qwen3-embedding:0.6b"));
    assert!(!server.called("/api/pull"));
}

#[test]
fn namespace_helpers() {
    assert!(is_ollama_official_namespace("nomic-embed-text"));
    assert!(is_ollama_official_namespace("qwen3-embedding:0.6b"));
    assert!(is_ollama_official_namespace("library/llama3:8b"));
    assert!(!is_ollama_official_namespace("user/custom-fork"));
    assert!(!is_ollama_official_namespace("user/custom-fork:latest"));
    assert!(!is_ollama_official_namespace(
        "registry.example.com/team/model:tag"
    ));
    assert!(!is_ollama_official_namespace("library/subdir/llama3"));

    assert_eq!(ollama_namespace_of("qwen3-embedding:0.6b"), None);
    assert_eq!(ollama_namespace_of("library/llama3:8b"), Some("library"));
    assert_eq!(ollama_namespace_of("user/custom-fork"), Some("user"));
    assert_eq!(
        ollama_namespace_of("registry.example.com/team/model:tag"),
        Some("registry.example.com/team")
    );
}

// --- num_ctx ---------------------------------------------------------------

async fn num_ctx_of(e: &OllamaEmbedder, server: &MockServer) -> serde_json::Value {
    e.embed("hello", TaskType::Document).await.unwrap();
    server.requests().last().unwrap().json()["options"]["num_ctx"].clone()
}

#[tokio::test]
async fn num_ctx_defaults_to_8192() {
    let server = MockServer::start(vec![ok(vec![0.1, 0.2, 0.3])]);
    let e = embedder(&server, "nomic-embed-text");
    assert_eq!(num_ctx_of(&e, &server).await, 8192);
}

#[tokio::test]
async fn num_ctx_from_the_constructor() {
    let server = MockServer::start(vec![ok(vec![0.1, 0.2, 0.3])]);
    let e = OllamaEmbedder::new(
        Some(&server.url),
        Some("nomic-embed-text"),
        None,
        Some(4096),
        None,
    );
    assert_eq!(num_ctx_of(&e, &server).await, 4096);
}

#[tokio::test]
async fn num_ctx_follows_the_model_context_length() {
    let server = MockServer::start(vec![
        show("nomic-bert", 768, 2048),
        tags("nomic-embed-text:latest", "sha256:abc"),
        ok(vec![0.0; 768]),
    ]);
    let e = embedder(&server, "nomic-embed-text");
    e.init().await.unwrap();
    assert_eq!(num_ctx_of(&e, &server).await, 2048);
}
