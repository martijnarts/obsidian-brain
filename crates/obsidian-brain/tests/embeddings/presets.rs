//! Ported from `presets.test.ts`.

use std::sync::Mutex;

use obsidian_brain::embeddings::presets::{
    DEPRECATED_PRESET_ALIASES, EMBEDDING_PRESETS, EmbeddingProvider, preset, reset_warnings,
    resolve_embedding_model, resolve_embedding_provider,
};

use crate::support::{capture_logs, env};

/// The one-shot warnings are process-wide, so the tests that reset and
/// count them run one at a time.
static WARNINGS: Mutex<()> = Mutex::new(());

fn model(pairs: &[(&str, &str)]) -> String {
    resolve_embedding_model(&env(pairs)).unwrap()
}

/// Resolve with fresh warnings, returning the log.
fn logged(runs: &[&[(&str, &str)]]) -> String {
    let _guard = WARNINGS.lock().unwrap_or_else(|e| e.into_inner());
    reset_warnings();
    capture_logs(|| {
        for pairs in runs {
            resolve_embedding_model(&env(pairs)).unwrap();
        }
    })
}

#[test]
fn canonical_presets() {
    assert_eq!(model(&[]), "Xenova/bge-small-en-v1.5");
    assert_eq!(
        model(&[("EMBEDDING_PRESET", "english")]),
        "Xenova/bge-small-en-v1.5"
    );
    assert_eq!(
        model(&[("EMBEDDING_PRESET", "english-fast")]),
        "MongoDB/mdbr-leaf-ir"
    );
    assert_eq!(
        model(&[("EMBEDDING_PRESET", "english-quality")]),
        "Xenova/bge-base-en-v1.5"
    );
    assert_eq!(
        model(&[("EMBEDDING_PRESET", "multilingual")]),
        "Xenova/multilingual-e5-small"
    );
    assert_eq!(
        model(&[("EMBEDDING_PRESET", "multilingual-ollama")]),
        "qwen3-embedding:0.6b"
    );
}

#[test]
fn multilingual_quality_preset() {
    let _guard = WARNINGS.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(
        model(&[("EMBEDDING_PRESET", "multilingual-quality")]),
        "Xenova/multilingual-e5-base"
    );
}

#[test]
fn preset_name_is_case_insensitive_and_trimmed() {
    assert_eq!(
        model(&[("EMBEDDING_PRESET", "MULTILINGUAL")]),
        "Xenova/multilingual-e5-small"
    );
    assert_eq!(
        model(&[("EMBEDDING_PRESET", "English-Quality")]),
        "Xenova/bge-base-en-v1.5"
    );
    assert_eq!(
        model(&[("EMBEDDING_PRESET", "  english  ")]),
        "Xenova/bge-small-en-v1.5"
    );
}

#[test]
fn embedding_model_wins_over_preset() {
    assert_eq!(
        model(&[
            ("EMBEDDING_MODEL", "custom/model"),
            ("EMBEDDING_PRESET", "english-fast")
        ]),
        "custom/model"
    );
    assert_eq!(
        model(&[("EMBEDDING_MODEL", "BAAI/bge-large-en-v1.5")]),
        "BAAI/bge-large-en-v1.5"
    );
    assert_eq!(
        model(&[
            ("EMBEDDING_MODEL", ""),
            ("EMBEDDING_PRESET", "english-fast")
        ]),
        "MongoDB/mdbr-leaf-ir"
    );
}

#[test]
fn fastest_alias_resolves_and_warns_once() {
    let _guard = WARNINGS.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(
        model(&[("EMBEDDING_PRESET", "fastest")]),
        "MongoDB/mdbr-leaf-ir"
    );
    drop(_guard);
    let log = logged(&[
        &[("EMBEDDING_PRESET", "fastest")],
        &[("EMBEDDING_PRESET", "fastest")],
    ]);
    assert!(
        log.contains("EMBEDDING_PRESET=\"fastest\" is deprecated"),
        "{log}"
    );
    assert!(log.contains("english-fast"));
    assert_eq!(log.matches("is deprecated").count(), 1);
}

#[test]
fn balanced_alias_is_a_model_change_and_warns_once() {
    let _guard = WARNINGS.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(
        model(&[("EMBEDDING_PRESET", "balanced")]),
        "Xenova/bge-small-en-v1.5"
    );
    drop(_guard);
    let log = logged(&[
        &[("EMBEDDING_PRESET", "balanced")],
        &[("EMBEDDING_PRESET", "balanced")],
    ]);
    assert!(
        log.contains("EMBEDDING_PRESET=\"balanced\" is deprecated"),
        "{log}"
    );
    assert!(log.contains("Xenova/bge-small-en-v1.5"));
    assert!(log.contains("Xenova/all-MiniLM-L6-v2"));
    assert!(log.contains("re-embed"));
    assert!(log.contains("different model"));
    assert!(log.contains("EMBEDDING_MODEL=Xenova/all-MiniLM-L6-v2"));
    assert_eq!(log.matches("EMBEDDING_PRESET=\"balanced\"").count(), 1);
}

#[test]
fn alias_table() {
    assert!(DEPRECATED_PRESET_ALIASES.contains(&("fastest", "english-fast")));
    assert!(DEPRECATED_PRESET_ALIASES.contains(&("balanced", "english")));
}

#[test]
fn unknown_preset_lists_the_valid_ones() {
    let err = resolve_embedding_model(&env(&[("EMBEDDING_PRESET", "englisch")]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("Unknown EMBEDDING_PRESET='englisch'"));
    assert!(err.contains(
        "Valid presets: english, english-fast, english-quality, multilingual, multilingual-quality, multilingual-ollama"
    ));
    let err = resolve_embedding_model(&env(&[("EMBEDDING_PRESET", "nope")]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("EMBEDDING_MODEL") && err.contains("power-user path"));
}

#[test]
fn providers() {
    let p = |pairs: &[(&str, &str)]| resolve_embedding_provider(&env(pairs)).unwrap();
    assert_eq!(
        p(&[("EMBEDDING_PRESET", "multilingual-ollama")]),
        EmbeddingProvider::Ollama
    );
    assert_eq!(
        p(&[("EMBEDDING_PRESET", "english")]),
        EmbeddingProvider::Transformers
    );
    let _guard = WARNINGS.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(
        p(&[
            ("EMBEDDING_PRESET", "multilingual-ollama"),
            ("EMBEDDING_PROVIDER", "transformers")
        ]),
        EmbeddingProvider::Transformers
    );
    drop(_guard);
    assert_eq!(
        p(&[("EMBEDDING_MODEL", "BAAI/bge-m3")]),
        EmbeddingProvider::Transformers
    );
}

#[test]
fn provider_mismatch_warns() {
    let log = logged(&[&[
        ("EMBEDDING_PRESET", "multilingual-ollama"),
        ("EMBEDDING_PROVIDER", "transformers"),
    ]]);
    assert!(
        log.contains(
            "EMBEDDING_PROVIDER='transformers' overrides EMBEDDING_PRESET='multilingual-ollama'"
        ),
        "{log}"
    );
}

#[test]
fn preset_table() {
    assert_eq!(EMBEDDING_PRESETS.len(), 6);
    let mo = preset("multilingual-ollama").unwrap();
    assert_eq!(mo.model, "qwen3-embedding:0.6b");
    assert_eq!(mo.provider, EmbeddingProvider::Ollama);
    assert_eq!(
        preset("english-fast").unwrap().model,
        "MongoDB/mdbr-leaf-ir"
    );
}

#[test]
fn multilingual_quality_warns_once_and_resets() {
    let log = logged(&[
        &[("EMBEDDING_PRESET", "multilingual-quality")],
        &[("EMBEDDING_PRESET", "multilingual-quality")],
    ]);
    assert!(
        log.contains("EMBEDDING_PRESET=\"multilingual-quality\""),
        "{log}"
    );
    assert!(log.contains("transformers.js#267"));
    assert!(log.contains("multilingual-ollama"));
    assert_eq!(log.matches("token_type_ids").count(), 1);

    let log = logged(&[&[("EMBEDDING_PRESET", "multilingual-quality")]]);
    assert!(log.contains("token_type_ids"));
}

#[test]
fn plain_multilingual_does_not_warn() {
    let log = logged(&[&[("EMBEDDING_PRESET", "multilingual")]]);
    assert!(!log.contains("token_type_ids"));
    assert!(!log.contains("transformers.js#267"));
}
