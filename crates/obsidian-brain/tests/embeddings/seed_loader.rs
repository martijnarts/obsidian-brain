//! Ported from `seed-loader.test.ts`.

use obsidian_brain::embeddings::seed_loader::{
    SeedEntry, adapt_entry, get_seed_meta, load_seed, parse_seed,
};
use serde_json::json;

#[test]
fn loads_a_non_empty_seed() {
    assert!(!load_seed().is_empty());
}

#[test]
fn contains_every_canonical_preset_model() {
    let seed = load_seed();
    for id in [
        "Xenova/bge-small-en-v1.5",
        "Xenova/bge-base-en-v1.5",
        "MongoDB/mdbr-leaf-ir",
        "Xenova/multilingual-e5-small",
        "Xenova/multilingual-e5-base",
        "qwen3-embedding:0.6b",
    ] {
        assert!(seed.contains_key(id), "seed missing canonical model {id}");
    }
}

#[test]
fn every_entry_has_a_positive_max_tokens() {
    for (id, entry) in load_seed().iter() {
        assert!(entry.max_tokens > 0, "{id} maxTokens must be > 0");
    }
}

#[test]
fn bge_is_asymmetric() {
    let seed = load_seed();
    let bge = &seed["Xenova/bge-small-en-v1.5"];
    assert_eq!(
        bge.query_prefix.as_deref(),
        Some("Represent this sentence for searching relevant passages: ")
    );
    assert_eq!(bge.document_prefix.as_deref(), Some(""));
}

#[test]
fn e5_has_query_and_passage_prefixes() {
    let seed = load_seed();
    let e5 = &seed["Xenova/multilingual-e5-small"];
    assert_eq!(e5.query_prefix.as_deref(), Some("query: "));
    assert_eq!(e5.document_prefix.as_deref(), Some("passage: "));
}

#[test]
fn exposes_seed_meta() {
    let meta = get_seed_meta().unwrap();
    assert!(meta.entries > 0);
    assert_eq!(meta.source.as_deref(), Some("mteb-2.12.30"));
}

#[test]
fn v1_adapter_keeps_the_three_fields() {
    let v1 = json!({
        "dim": 384,
        "maxTokens": 512,
        "queryPrefix": "Represent this sentence for searching relevant passages: ",
        "documentPrefix": "",
        "prefixSource": "metadata",
        "modelType": "bert",
        "baseModel": "BAAI/bge-small-en-v1.5",
        "hasDenseLayer": false,
        "hasNormalize": true,
        "sizeBytes": 35200000,
        "runnableViaTransformersJs": true,
    });
    assert_eq!(
        adapt_entry(&v1),
        Some(SeedEntry {
            max_tokens: 512,
            query_prefix: Some("Represent this sentence for searching relevant passages: ".into()),
            document_prefix: Some(String::new()),
        })
    );
}

#[test]
fn v1_adapter_keeps_null_prefixes() {
    let v1 = json!({
        "dim": 1024, "maxTokens": 8192, "queryPrefix": null, "documentPrefix": null,
        "prefixSource": "none", "modelType": "xlm-roberta", "baseModel": null,
        "hasDenseLayer": false, "hasNormalize": true, "sizeBytes": null,
        "runnableViaTransformersJs": false,
    });
    let e = adapt_entry(&v1).unwrap();
    assert_eq!(
        (e.max_tokens, e.query_prefix, e.document_prefix),
        (8192, None, None)
    );
}

#[test]
fn bad_files_yield_an_empty_seed() {
    assert!(parse_seed("not json").0.is_empty());
    assert!(parse_seed(r#"{"$schemaVersion": 9, "models": {"a": {"maxTokens": 1, "queryPrefix": null, "documentPrefix": null}}}"#).0.is_empty());
    assert!(parse_seed(r#"{"$schemaVersion": 2}"#).0.is_empty());
}

#[test]
fn invalid_entries_are_skipped() {
    let (seed, meta) = parse_seed(
        r#"{"$schemaVersion": 1, "$mtebRevision": "r1", "models": {
            "ok": {"maxTokens": 512, "queryPrefix": "q: ", "documentPrefix": null},
            "zero": {"maxTokens": 0, "queryPrefix": null, "documentPrefix": null},
            "badprefix": {"maxTokens": 5, "queryPrefix": 3, "documentPrefix": null}
        }}"#,
    );
    assert_eq!(seed.len(), 1);
    assert!(seed.contains_key("ok"));
    let meta = meta.unwrap();
    assert_eq!(meta.source.as_deref(), Some("r1"));
    assert_eq!(meta.entries, 1);
}
