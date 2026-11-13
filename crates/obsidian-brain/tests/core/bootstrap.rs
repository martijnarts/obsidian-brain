//! Tests of the startup compatibility check and the reindex after a model
//! change. The index rebuilds itself on a layout change.

use obsidian_brain::pipeline::{BootstrapResult, bootstrap};
use obsidian_brain::store::Db;
use obsidian_brain::store::embeddings::upsert_embedding;
use obsidian_brain::store::metadata::{get_metadata, set_metadata};
use obsidian_brain::store::nodes::upsert_node;
use obsidian_brain::types::ParsedNode;

use crate::support::{FakeEmbedder, memory_db};

const BGE_QUERY: &str = "Represent this sentence for searching relevant passages: ";

fn boot(db: &Db, e: &FakeEmbedder) -> BootstrapResult {
    db.with(|c| bootstrap(c, e)).unwrap()
}

fn meta(db: &Db, key: &str) -> Option<String> {
    db.with(|c| get_metadata(c, key)).unwrap()
}

fn node(id: &str) -> ParsedNode {
    ParsedNode {
        id: id.into(),
        title: id.trim_end_matches(".md").into(),
        content: "body".into(),
        frontmatter: Default::default(),
    }
}

fn unit_vec(dim: usize) -> Vec<f32> {
    let mut v: Vec<f32> = (0..dim).map(|i| (i as f32).sin() * 0.5).collect();
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.iter_mut().for_each(|x| *x /= n);
    v
}

#[test]
fn stamps_metadata_on_first_boot_without_a_reindex() {
    let db = memory_db();
    let r = boot(
        &db,
        &FakeEmbedder::stub("Xenova/all-MiniLM-L6-v2", 384, "stub"),
    );
    assert!(!r.needs_reindex);
    assert_eq!(
        meta(&db, "embedding_model").as_deref(),
        Some("Xenova/all-MiniLM-L6-v2")
    );
    assert_eq!(meta(&db, "embedding_dim").as_deref(), Some("384"));
    assert!(meta(&db, "schema_version").is_some());
    assert_eq!(meta(&db, "embedder_provider").as_deref(), Some("stub"));
}

#[test]
fn is_idempotent_for_an_unchanged_embedder() {
    let db = memory_db();
    let e = FakeEmbedder::stub("Xenova/all-MiniLM-L6-v2", 384, "stub");
    boot(&db, &e);
    assert_eq!(boot(&db, &e), BootstrapResult::default());
}

#[test]
fn a_model_change_wipes_the_embedding_state_and_reindexes() {
    let db = memory_db();
    boot(
        &db,
        &FakeEmbedder::stub("Xenova/all-MiniLM-L6-v2", 384, "stub"),
    );
    db.with(|c| {
        upsert_node(c, &node("n.md")).unwrap();
        upsert_embedding(c, "n.md", &unit_vec(384)).unwrap();
        c.execute(
            "INSERT INTO sync (path, mtime, indexed_at) VALUES ('n.md', 1000, 2000)",
            [],
        )
        .unwrap();
    });
    let r = boot(
        &db,
        &FakeEmbedder::stub("Xenova/bge-small-en-v1.5", 384, "stub"),
    );
    assert!(r.needs_reindex);
    assert!(
        r.reasons
            .iter()
            .any(|r| r.contains("embedding model changed"))
    );
    let sync: i64 = db
        .with(|c| c.query_row("SELECT COUNT(*) FROM sync", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(sync, 0);
    assert_eq!(
        meta(&db, "embedding_model").as_deref(),
        Some("Xenova/bge-small-en-v1.5")
    );
}

#[test]
fn a_dim_change_reindexes_under_the_same_name() {
    let db = memory_db();
    boot(&db, &FakeEmbedder::stub("custom-model", 384, "stub"));
    let r = boot(&db, &FakeEmbedder::stub("custom-model", 768, "stub"));
    assert!(r.needs_reindex);
    assert_eq!(meta(&db, "embedding_dim").as_deref(), Some("768"));
}

#[test]
fn nodes_without_chunks_reindex() {
    let db = memory_db();
    let e = FakeEmbedder::stub("Xenova/all-MiniLM-L6-v2", 384, "stub");
    boot(&db, &e);
    db.with(|c| upsert_node(c, &node("legacy.md"))).unwrap();
    let r = boot(&db, &e);
    assert!(r.needs_reindex);
    assert!(
        r.reasons
            .iter()
            .any(|r| r.contains("paragraph-level indexing"))
    );
}

#[test]
fn rebuilds_the_fts_index_when_the_tokenizer_changed() {
    let db = memory_db();
    db.with(|c| {
        c.execute_batch(
            "DROP TABLE nodes_fts;
             CREATE VIRTUAL TABLE nodes_fts USING fts5(title, content, content='nodes', content_rowid='rowid');",
        )
        .unwrap();
        upsert_node(c, &node("a.md")).unwrap();
    });
    let r = boot(
        &db,
        &FakeEmbedder::stub("Xenova/all-MiniLM-L6-v2", 384, "stub"),
    );
    assert!(
        r.reasons
            .iter()
            .any(|r| r.contains("full-text search tokenization"))
    );
    let sql: String = db
        .with(|c| {
            c.query_row(
                "SELECT sql FROM sqlite_master WHERE name = 'nodes_fts'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert!(sql.contains("porter unicode61"));
}

#[test]
fn a_symmetric_model_never_reindexes_for_its_prefix_strategy() {
    let db = memory_db();
    let e = FakeEmbedder::stub("Xenova/all-MiniLM-L6-v2", 384, "transformers.js");
    boot(&db, &e);
    let r = boot(&db, &e);
    assert!(!r.needs_reindex);
    assert!(r.reasons.iter().all(|r| !r.contains("prefix")));
}

#[test]
fn a_missing_prefix_strategy_of_an_asymmetric_model_reindexes() {
    let db = memory_db();
    let e = FakeEmbedder::stub("Xenova/bge-small-en-v1.5", 384, "transformers.js")
        .with_prefixes(BGE_QUERY, "");
    boot(&db, &e);
    db.with(|c| {
        c.execute(
            "DELETE FROM index_metadata WHERE key = 'embedder_prefix_strategy'",
            [],
        )
    })
    .unwrap();
    let r = boot(&db, &e);
    assert!(r.needs_reindex);
    assert!(!r.reasons.is_empty());
}

#[test]
fn a_stale_prefix_strategy_reindexes() {
    let db = memory_db();
    let e = FakeEmbedder::stub("Xenova/bge-small-en-v1.5", 384, "transformers.js")
        .with_prefixes(BGE_QUERY, "");
    boot(&db, &e);
    db.with(|c| set_metadata(c, "embedder_prefix_strategy", "stale_v1_hash_abcdef01"))
        .unwrap();
    let r = boot(&db, &e);
    assert!(r.needs_reindex);
    assert!(
        r.reasons
            .iter()
            .any(|r| r.contains("different query/document prefixes"))
    );
}

#[test]
fn turning_symmetric_reindexes() {
    let db = memory_db();
    let asym = FakeEmbedder::stub("m", 384, "transformers.js").with_prefixes(BGE_QUERY, "");
    boot(&db, &asym);
    let r = boot(&db, &FakeEmbedder::stub("m", 384, "transformers.js"));
    assert!(r.needs_reindex);
    assert!(
        r.reasons
            .iter()
            .any(|r| r.contains("treats queries and documents the same way"))
    );
}

#[test]
fn bge_to_minilm_reindexes_for_the_model_change() {
    let db = memory_db();
    boot(
        &db,
        &FakeEmbedder::stub("Xenova/bge-small-en-v1.5", 384, "transformers.js"),
    );
    let r = boot(
        &db,
        &FakeEmbedder::stub("Xenova/all-MiniLM-L6-v2", 384, "transformers.js"),
    );
    assert!(r.needs_reindex);
    assert!(
        r.reasons
            .iter()
            .any(|r| r.contains("embedding model changed"))
    );
}

#[test]
fn an_ollama_embedder_without_metadata_needs_no_prefix_reindex() {
    let db = memory_db();
    let e = FakeEmbedder::stub("bge-small-en-v1.5", 384, "ollama");
    boot(&db, &e);
    let r = boot(&db, &e);
    assert!(!r.needs_reindex);
}

#[test]
fn the_capability_and_failed_chunk_tables_exist_after_a_boot() {
    let db = memory_db();
    let e = FakeEmbedder::stub("Xenova/bge-small-en-v1.5", 384, "transformers.js");
    boot(&db, &e);
    boot(&db, &e);
    let n: i64 = db
        .with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('embedder_capability','failed_chunks')",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(n, 2);
}

/// Seed the metadata as a previous boot left it.
fn seed(db: &Db, model: &str, dim: usize, provider: &'static str) {
    boot(db, &FakeEmbedder::stub(model, dim, provider));
    db.with(|c| {
        set_metadata(c, "embedding_model", model)?;
        set_metadata(c, "embedding_dim", &dim.to_string())?;
        set_metadata(c, "embedder_provider", provider)
    })
    .unwrap();
}

#[test]
fn model_change_scenarios() {
    // A: a clean swap to another name and dim.
    let db = memory_db();
    seed(&db, "Xenova/multilingual-e5-small", 384, "transformers.js");
    let r = boot(
        &db,
        &FakeEmbedder::stub("ollama:nomic-embed-text", 768, "ollama"),
    );
    assert!(r.needs_reindex);
    assert!(
        r.reasons
            .iter()
            .any(|r| r.contains("embedding model changed")
                && r.contains("Xenova/multilingual-e5-small")
                && r.contains("ollama:nomic-embed-text"))
    );
    assert_eq!(
        meta(&db, "embedding_model").as_deref(),
        Some("ollama:nomic-embed-text")
    );
    assert_eq!(meta(&db, "embedding_dim").as_deref(), Some("768"));

    // B: a partial boot left the new name with a stale dim.
    let db = memory_db();
    seed(&db, "ollama:nomic-embed-text", 1024, "ollama");
    let r = boot(
        &db,
        &FakeEmbedder::stub("ollama:nomic-embed-text", 768, "ollama"),
    );
    assert!(r.needs_reindex);
    assert_eq!(meta(&db, "embedding_dim").as_deref(), Some("768"));

    // C and E: same model and dim, whatever preset named it.
    let db = memory_db();
    seed(&db, "Xenova/bge-small-en-v1.5", 384, "transformers.js");
    let r = boot(
        &db,
        &FakeEmbedder::stub("Xenova/bge-small-en-v1.5", 384, "transformers.js"),
    );
    assert!(!r.needs_reindex);
    assert!(
        r.reasons
            .iter()
            .all(|r| !r.contains("embedding model changed"))
    );

    // D: nodes but an empty chunks table.
    let db = memory_db();
    seed(&db, "Xenova/bge-small-en-v1.5", 384, "transformers.js");
    db.with(|c| upsert_node(c, &node("note.md"))).unwrap();
    let r = boot(
        &db,
        &FakeEmbedder::stub("Xenova/bge-small-en-v1.5", 384, "transformers.js"),
    );
    assert!(r.needs_reindex);
    assert!(
        r.reasons
            .iter()
            .any(|r| r.contains("paragraph-level indexing"))
    );

    // F: the same weights through another provider are another identifier.
    let db = memory_db();
    seed(&db, "bge-m3", 1024, "transformers.js");
    let r = boot(&db, &FakeEmbedder::stub("ollama:bge-m3", 1024, "ollama"));
    assert!(r.needs_reindex);
    assert!(
        r.reasons
            .iter()
            .any(|r| r.contains("embedding model changed") && r.contains("ollama:bge-m3"))
    );
    assert_eq!(
        meta(&db, "embedding_model").as_deref(),
        Some("ollama:bge-m3")
    );
}
