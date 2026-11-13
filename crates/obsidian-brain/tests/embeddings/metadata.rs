//! Tests of the model metadata cache and the metadata resolver chain.

use obsidian_brain::embeddings::PrefixSource;
use obsidian_brain::embeddings::metadata_cache::{
    CachedMetadata, load_cached_metadata, model_hash, upsert_cached_metadata,
};
use obsidian_brain::embeddings::metadata_resolver::{
    ResolvedFrom, ResolverDeps, resolve_model_metadata, resolve_model_metadata_sync,
};
use obsidian_brain::embeddings::overrides::{ModelOverride, Overrides};
use obsidian_brain::embeddings::seed_loader::{Seed, SeedEntry};
use rusqlite::{Connection, params};

use crate::support::{StubEmbedder, open_test_db};

const BGE_QUERY: &str = "Represent this sentence for searching relevant passages: ";

fn seed_entry() -> SeedEntry {
    SeedEntry {
        max_tokens: 512,
        query_prefix: Some(BGE_QUERY.into()),
        document_prefix: Some(String::new()),
    }
}

fn seed(ids: &[&str]) -> Seed {
    ids.iter()
        .map(|id| (id.to_string(), seed_entry()))
        .collect()
}

fn row(model_id: &str) -> CachedMetadata {
    CachedMetadata {
        model_id: model_id.into(),
        dim: Some(384),
        max_tokens: Some(512),
        query_prefix: None,
        document_prefix: None,
        prefix_source: PrefixSource::Fallback,
        base_model: None,
        size_bytes: None,
        fetched_at: Some(1_700_000_000_000),
    }
}

fn deps<'a>(conn: &'a Connection, seed: &'a Seed, overrides: &'a Overrides) -> ResolverDeps<'a> {
    ResolverDeps {
        conn,
        embedder: None,
        seed: Some(seed),
        overrides: Some(overrides),
    }
}

// --- metadata cache --------------------------------------------------------

#[test]
fn cache_round_trips() {
    let conn = open_test_db();
    let meta = CachedMetadata {
        model_id: "BAAI/bge-small-en-v1.5".into(),
        dim: Some(384),
        max_tokens: Some(512),
        query_prefix: Some(BGE_QUERY.into()),
        document_prefix: Some(String::new()),
        prefix_source: PrefixSource::Seed,
        base_model: None,
        size_bytes: Some(35_200_000),
        fetched_at: Some(1_700_000_000_000),
    };
    upsert_cached_metadata(&conn, &meta).unwrap();
    assert_eq!(
        load_cached_metadata(&conn, "BAAI/bge-small-en-v1.5").unwrap(),
        Some(meta)
    );
}

#[test]
fn cache_miss_for_an_absent_model() {
    let conn = open_test_db();
    assert_eq!(load_cached_metadata(&conn, "never/seen").unwrap(), None);
}

#[test]
fn capacity_only_rows_read_as_a_miss() {
    let conn = open_test_db();
    conn.execute(
        "INSERT INTO embedder_capability (embedder_id, model_hash, advertised_max_tokens, discovered_max_tokens, discovered_at, method) VALUES (?, ?, ?, ?, ?, ?)",
        params!["legacy/model", model_hash("legacy/model"), 512, 512, 1_700_000_000_000i64, "tokenizer_config"],
    )
    .unwrap();
    assert_eq!(load_cached_metadata(&conn, "legacy/model").unwrap(), None);
}

#[test]
fn upsert_overwrites_metadata_but_keeps_the_ratchet() {
    let conn = open_test_db();
    upsert_cached_metadata(
        &conn,
        &CachedMetadata {
            query_prefix: Some("old: ".into()),
            fetched_at: Some(1000),
            ..row("a/b")
        },
    )
    .unwrap();
    conn.execute(
        "UPDATE embedder_capability SET discovered_max_tokens = 300 WHERE embedder_id = 'a/b'",
        [],
    )
    .unwrap();
    let new = CachedMetadata {
        model_id: "a/b".into(),
        dim: Some(768),
        max_tokens: Some(8192),
        query_prefix: Some("new: ".into()),
        document_prefix: Some("doc: ".into()),
        prefix_source: PrefixSource::Metadata,
        base_model: Some("upstream/x".into()),
        size_bytes: Some(12345),
        fetched_at: Some(2000),
    };
    upsert_cached_metadata(&conn, &new).unwrap();
    assert_eq!(load_cached_metadata(&conn, "a/b").unwrap(), Some(new));
    let discovered: i64 = conn
        .query_row(
            "SELECT discovered_max_tokens FROM embedder_capability WHERE embedder_id = 'a/b'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(discovered, 300);
}

#[test]
fn model_hash_is_32_hex_chars_of_sha256() {
    assert_eq!(model_hash("a/b").len(), 32);
    assert_eq!(model_hash("a/b"), model_hash("a/b"));
    assert_ne!(model_hash("a/b"), model_hash("a/c"));
}

// --- resolver --------------------------------------------------------------

#[test]
fn step_1_cache_hit_even_when_very_old() {
    let conn = open_test_db();
    upsert_cached_metadata(
        &conn,
        &CachedMetadata {
            dim: Some(1024),
            max_tokens: Some(8192),
            query_prefix: Some("q: ".into()),
            document_prefix: Some("d: ".into()),
            prefix_source: PrefixSource::Metadata,
            fetched_at: Some(1),
            ..row("cached/model")
        },
    )
    .unwrap();
    let (s, o) = (seed(&[]), Overrides::new());
    let r = resolve_model_metadata("cached/model", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(r.resolved_from, ResolvedFrom::Cache);
    assert_eq!(r.dim, Some(1024));
}

#[test]
fn step_2_seed_hit_warms_the_cache() {
    let conn = open_test_db();
    let (s, o) = (seed(&["seed/model"]), Overrides::new());
    let r = resolve_model_metadata("seed/model", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(r.resolved_from, ResolvedFrom::Seed);
    assert_eq!(r.query_prefix, BGE_QUERY);
    let second = resolve_model_metadata("seed/model", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(second.resolved_from, ResolvedFrom::Cache);
}

#[test]
fn step_3_seed_miss_with_embedder_uses_its_dim() {
    let conn = open_test_db();
    let (s, o) = (seed(&[]), Overrides::new());
    let embedder = StubEmbedder::new("byom/exotic", 256);
    let d = ResolverDeps {
        embedder: Some(&embedder),
        ..deps(&conn, &s, &o)
    };
    let r = resolve_model_metadata("byom/exotic", &d).unwrap();
    assert_eq!(r.resolved_from, ResolvedFrom::EmbedderProbe);
    assert_eq!(r.dim, Some(256));
    assert_eq!(
        (r.query_prefix.as_str(), r.document_prefix.as_str()),
        ("", "")
    );
    assert_eq!(r.max_tokens, 512);
}

#[test]
fn step_4_seed_miss_without_embedder_uses_defaults() {
    let conn = open_test_db();
    let (s, o) = (seed(&[]), Overrides::new());
    let r = resolve_model_metadata("byom/no-embedder", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(r.resolved_from, ResolvedFrom::Fallback);
    assert_eq!(r.max_tokens, 512);
    assert_eq!(r.dim, None);
    assert_eq!(r.prefix_source, PrefixSource::Fallback);
}

fn one_override(id: &str, o: ModelOverride) -> Overrides {
    Overrides::from([(id.to_string(), o)])
}

#[test]
fn override_max_tokens_patches_the_seed() {
    let conn = open_test_db();
    let s = seed(&["seed/m"]);
    let o = one_override(
        "seed/m",
        ModelOverride {
            max_tokens: Some(8192),
            ..ModelOverride::default()
        },
    );
    let r = resolve_model_metadata("seed/m", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(r.resolved_from, ResolvedFrom::Seed);
    assert_eq!(r.max_tokens, 8192);
    assert_eq!(r.query_prefix, BGE_QUERY);
    assert_eq!(r.prefix_source, PrefixSource::Override);
    assert!(r.override_applied);
}

#[test]
fn override_query_prefix_replaces_only_that_field() {
    let conn = open_test_db();
    let s = seed(&["seed/m"]);
    let o = one_override(
        "seed/m",
        ModelOverride {
            query_prefix: Some(Some("CUSTOM: ".into())),
            ..ModelOverride::default()
        },
    );
    let r = resolve_model_metadata("seed/m", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(r.query_prefix, "CUSTOM: ");
    assert_eq!(r.document_prefix, "");
    assert_eq!(r.max_tokens, 512);
    assert!(r.override_applied);
}

#[test]
fn override_null_prefix_clears_it() {
    let conn = open_test_db();
    let s = seed(&["seed/m"]);
    let o = one_override(
        "seed/m",
        ModelOverride {
            query_prefix: Some(None),
            ..ModelOverride::default()
        },
    );
    let r = resolve_model_metadata("seed/m", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(r.query_prefix, "");
    assert!(r.override_applied);
}

#[test]
fn no_override_keeps_the_step_source() {
    let conn = open_test_db();
    let (s, o) = (seed(&["seed/m"]), Overrides::new());
    let r = resolve_model_metadata("seed/m", &deps(&conn, &s, &o)).unwrap();
    assert!(!r.override_applied);
    assert_eq!(r.prefix_source, PrefixSource::Seed);
}

#[test]
fn complete_override_short_circuits() {
    let conn = open_test_db();
    let s = seed(&[]);
    let o = one_override(
        "user/added-model",
        ModelOverride {
            max_tokens: Some(4096),
            query_prefix: Some(Some("Q: ".into())),
            document_prefix: Some(Some("D: ".into())),
        },
    );
    let r = resolve_model_metadata("user/added-model", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(r.resolved_from, ResolvedFrom::Cache);
    assert_eq!(r.max_tokens, 4096);
    assert_eq!(r.query_prefix, "Q: ");
    assert_eq!(r.document_prefix, "D: ");
    assert_eq!(r.prefix_source, PrefixSource::Override);
}

#[test]
fn partial_override_still_runs_the_chain() {
    let conn = open_test_db();
    let s = seed(&[]);
    let o = one_override(
        "user/partial",
        ModelOverride {
            max_tokens: Some(9999),
            ..ModelOverride::default()
        },
    );
    let r = resolve_model_metadata("user/partial", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(r.resolved_from, ResolvedFrom::Fallback);
    assert_eq!(r.max_tokens, 9999);
    assert_eq!(r.query_prefix, "");
}

#[test]
fn override_applies_on_a_cache_hit() {
    let conn = open_test_db();
    upsert_cached_metadata(
        &conn,
        &CachedMetadata {
            max_tokens: Some(256),
            query_prefix: Some("orig-q: ".into()),
            document_prefix: Some(String::new()),
            prefix_source: PrefixSource::Metadata,
            ..row("cached/m")
        },
    )
    .unwrap();
    let s = seed(&[]);
    let o = one_override(
        "cached/m",
        ModelOverride {
            query_prefix: Some(Some("overridden: ".into())),
            ..ModelOverride::default()
        },
    );
    let r = resolve_model_metadata("cached/m", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(r.resolved_from, ResolvedFrom::Cache);
    assert_eq!(r.query_prefix, "overridden: ");
    assert!(r.override_applied);
}

#[test]
fn stale_null_prefix_row_is_promoted_from_the_seed() {
    let conn = open_test_db();
    upsert_cached_metadata(&conn, &row("asym/model")).unwrap();
    let (s, o) = (seed(&["asym/model"]), Overrides::new());
    let r = resolve_model_metadata("asym/model", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(r.resolved_from, ResolvedFrom::Seed);
    assert_eq!(r.query_prefix, BGE_QUERY);
    let second = resolve_model_metadata("asym/model", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(second.resolved_from, ResolvedFrom::Cache);
    assert_eq!(second.query_prefix, BGE_QUERY);
}

#[test]
fn stale_row_without_a_seed_entry_stays() {
    let conn = open_test_db();
    upsert_cached_metadata(&conn, &row("unknown/model")).unwrap();
    let (s, o) = (seed(&[]), Overrides::new());
    let r = resolve_model_metadata("unknown/model", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(r.resolved_from, ResolvedFrom::Cache);
    assert_eq!(r.query_prefix, "");
}

#[test]
fn row_with_a_prefix_is_not_promoted() {
    let conn = open_test_db();
    upsert_cached_metadata(
        &conn,
        &CachedMetadata {
            query_prefix: Some("cached-q: ".into()),
            document_prefix: Some("cached-d: ".into()),
            prefix_source: PrefixSource::Metadata,
            ..row("good/model")
        },
    )
    .unwrap();
    let (s, o) = (seed(&["good/model"]), Overrides::new());
    let r = resolve_model_metadata("good/model", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(r.resolved_from, ResolvedFrom::Cache);
    assert_eq!(r.query_prefix, "cached-q: ");
}

#[test]
fn partial_override_on_a_promoted_row() {
    let conn = open_test_db();
    upsert_cached_metadata(&conn, &row("asym/model")).unwrap();
    let s = seed(&["asym/model"]);
    let o = one_override(
        "asym/model",
        ModelOverride {
            query_prefix: Some(Some("user-override: ".into())),
            ..ModelOverride::default()
        },
    );
    let r = resolve_model_metadata("asym/model", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(r.resolved_from, ResolvedFrom::Seed);
    assert_eq!(r.query_prefix, "user-override: ");
    assert_eq!(r.document_prefix, "");
    assert!(r.override_applied);
}

#[test]
fn ollama_identifier_uses_the_bare_seed_key() {
    let conn = open_test_db();
    let (s, o) = (seed(&["qwen3-embedding:0.6b"]), Overrides::new());
    let r = resolve_model_metadata("ollama:qwen3-embedding:0.6b", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(r.resolved_from, ResolvedFrom::Seed);
    assert_eq!(r.query_prefix, BGE_QUERY);
    let second =
        resolve_model_metadata("ollama:qwen3-embedding:0.6b", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(second.resolved_from, ResolvedFrom::Cache);
}

#[test]
fn stale_ollama_row_is_promoted_from_the_bare_seed_key() {
    let conn = open_test_db();
    upsert_cached_metadata(
        &conn,
        &CachedMetadata {
            dim: Some(1024),
            max_tokens: Some(32768),
            ..row("ollama:qwen3-embedding:0.6b")
        },
    )
    .unwrap();
    let (s, o) = (seed(&["qwen3-embedding:0.6b"]), Overrides::new());
    let r = resolve_model_metadata("ollama:qwen3-embedding:0.6b", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(r.resolved_from, ResolvedFrom::Seed);
    assert_eq!(r.query_prefix, BGE_QUERY);
}

#[test]
fn byom_ollama_model_falls_to_the_probe() {
    let conn = open_test_db();
    let (s, o) = (seed(&[]), Overrides::new());
    let mut embedder = StubEmbedder::new("ollama:my-byom-model", 384);
    embedder.provider = "ollama";
    let d = ResolverDeps {
        embedder: Some(&embedder),
        ..deps(&conn, &s, &o)
    };
    let r = resolve_model_metadata("ollama:my-byom-model", &d).unwrap();
    assert_eq!(r.resolved_from, ResolvedFrom::EmbedderProbe);
    assert_eq!(r.dim, Some(384));
}

#[test]
fn sync_returns_a_cache_hit() {
    let conn = open_test_db();
    upsert_cached_metadata(
        &conn,
        &CachedMetadata {
            query_prefix: Some("q: ".into()),
            document_prefix: Some("d: ".into()),
            prefix_source: PrefixSource::Seed,
            ..row("cached/model")
        },
    )
    .unwrap();
    let (s, o) = (seed(&[]), Overrides::new());
    let r = resolve_model_metadata_sync("cached/model", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(r.unwrap().resolved_from, ResolvedFrom::Cache);
}

#[test]
fn sync_returns_none_without_cache_or_seed() {
    let conn = open_test_db();
    let (s, o) = (seed(&[]), Overrides::new());
    assert_eq!(
        resolve_model_metadata_sync("never/seen", &deps(&conn, &s, &o)).unwrap(),
        None
    );
}

#[test]
fn sync_seed_hit_warms_the_cache() {
    let conn = open_test_db();
    let o = Overrides::new();
    let s = seed(&["seed/model"]);
    let first = resolve_model_metadata_sync("seed/model", &deps(&conn, &s, &o)).unwrap();
    assert_eq!(first.unwrap().resolved_from, ResolvedFrom::Seed);
    let empty = seed(&[]);
    let second = resolve_model_metadata_sync("seed/model", &deps(&conn, &empty, &o)).unwrap();
    assert_eq!(second.unwrap().resolved_from, ResolvedFrom::Cache);
}

#[test]
fn sync_promotes_a_stale_row() {
    let conn = open_test_db();
    upsert_cached_metadata(&conn, &row("asym/model")).unwrap();
    let (s, o) = (seed(&["asym/model"]), Overrides::new());
    let r = resolve_model_metadata_sync("asym/model", &deps(&conn, &s, &o))
        .unwrap()
        .unwrap();
    assert_eq!(r.resolved_from, ResolvedFrom::Seed);
    assert_eq!(r.query_prefix, BGE_QUERY);
}

#[test]
fn resolved_metadata_converts_for_the_embedder() {
    let conn = open_test_db();
    let (s, o) = (seed(&["seed/m"]), Overrides::new());
    let m = resolve_model_metadata("seed/m", &deps(&conn, &s, &o))
        .unwrap()
        .to_embedder_metadata();
    assert_eq!(m.model_id, "seed/m");
    assert_eq!(m.query_prefix, BGE_QUERY);
    assert_eq!(m.prefix_source, PrefixSource::Seed);
}
