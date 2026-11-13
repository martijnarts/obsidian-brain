//! The read and validation half of `overrides.test.ts`. The write half
//! (`saveOverride`, `removeOverride`) served only the dropped `models` CLI.

use obsidian_brain::embeddings::overrides::{ModelOverride, load_overrides_from};
use serde_json::json;

fn write(dir: &tempfile::TempDir, body: &str) -> std::path::PathBuf {
    let path = dir.path().join("model-overrides.json");
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn missing_file_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert!(load_overrides_from(&dir.path().join("model-overrides.json")).is_empty());
}

#[test]
fn reads_a_valid_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        &dir,
        &json!({"$version": 1, "models": {
            "MongoDB/mdbr-leaf-ir": {"maxTokens": 1024},
            "foo/bar": {"maxTokens": 512, "queryPrefix": "q: "}
        }})
        .to_string(),
    );
    let map = load_overrides_from(&path);
    assert_eq!(map.len(), 2);
    assert_eq!(
        map["MongoDB/mdbr-leaf-ir"],
        ModelOverride {
            max_tokens: Some(1024),
            ..ModelOverride::default()
        }
    );
    assert_eq!(map["foo/bar"].query_prefix, Some(Some("q: ".into())));
}

#[test]
fn rejects_non_positive_max_tokens_but_keeps_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        &dir,
        &json!({"$version": 1, "models": {"foo/bar": {"maxTokens": -5, "queryPrefix": "q: "}}})
            .to_string(),
    );
    let ov = &load_overrides_from(&path)["foo/bar"];
    assert_eq!(ov.max_tokens, None);
    assert_eq!(ov.query_prefix, Some(Some("q: ".into())));
}

#[test]
fn rejects_non_string_prefixes() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        &dir,
        &json!({"$version": 1, "models": {"foo/bar": {"queryPrefix": {"wrong": "shape"}, "maxTokens": 512}}})
            .to_string(),
    );
    let ov = &load_overrides_from(&path)["foo/bar"];
    assert_eq!(ov.query_prefix, None);
    assert_eq!(ov.max_tokens, Some(512));
}

#[test]
fn null_prefix_means_clear() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        &dir,
        &json!({"$version": 1, "models": {"foo/bar": {"queryPrefix": null}}}).to_string(),
    );
    assert_eq!(
        load_overrides_from(&path)["foo/bar"].query_prefix,
        Some(None)
    );
}

#[test]
fn fractional_max_tokens_floor() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        &dir,
        &json!({"$version": 1, "models": {"foo/bar": {"maxTokens": 511.9}}}).to_string(),
    );
    assert_eq!(load_overrides_from(&path)["foo/bar"].max_tokens, Some(511));
}

#[test]
fn ignores_an_unsupported_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        &dir,
        &json!({"$version": 99, "models": {"foo/bar": {"maxTokens": 1}}}).to_string(),
    );
    assert!(load_overrides_from(&path).is_empty());
}

#[test]
fn ignores_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "{ not valid json");
    assert!(load_overrides_from(&path).is_empty());
}
