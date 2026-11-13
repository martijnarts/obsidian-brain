//! Embeds the 50 files in `tests/fixtures/embedder-v3-reference/` and checks
//! each vector against the recorded baseline in
//! `tests/fixtures/embedder-v4-baseline.json` at cosine >= 0.99.
//!
//! 0.99, not 0.9999: q8 inference drifts across SIMD backends by about
//! 0.001 cosine. Real regressions (tokenizer, pooling, weights, wrong
//! model) land far below 0.99.
//!
//! Downloads the model (~34 MB), so it is ignored by default:
//! `cargo test --test embeddings_v4_equivalence -- --ignored`.

use std::path::PathBuf;

use obsidian_brain::embeddings::embedder::{TransformersEmbedder, TransformersEmbedderOptions};
use obsidian_brain::embeddings::{Embedder, TaskType};
use serde_json::Value;

const COSINE_THRESHOLD: f64 = 0.99;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn cache_dir() -> PathBuf {
    std::env::var_os("TRANSFORMERS_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("hf-cache"))
}

fn cosine(a: &[f32], b: &[f64]) -> f64 {
    assert_eq!(a.len(), b.len(), "dimension mismatch");
    let (mut dot, mut na, mut nb) = (0f64, 0f64, 0f64);
    for (x, y) in a.iter().zip(b) {
        let x = f64::from(*x);
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom == 0.0 { 0.0 } else { dot / denom }
}

#[tokio::test]
#[ignore = "downloads Xenova/bge-small-en-v1.5 from Hugging Face"]
async fn embedder_v4_equivalence() {
    let root = fixtures();
    let baseline: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("embedder-v4-baseline.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(baseline["model"], "Xenova/bge-small-en-v1.5");
    assert_eq!(baseline["dtype"], "q8");

    let fixture_dir = root.join("embedder-v3-reference");
    let mut files: Vec<String> = std::fs::read_dir(&fixture_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|f| f.ends_with(".md"))
        .collect();
    files.sort();
    let vectors = baseline["vectors"].as_object().unwrap();
    let mut keys: Vec<&String> = vectors.keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        files.iter().collect::<Vec<_>>(),
        "baseline covers every fixture"
    );

    let embedder = TransformersEmbedder::new(
        Some("Xenova/bge-small-en-v1.5"),
        TransformersEmbedderOptions {
            idle_unload_ms: Some(0),
            cache_dir: Some(cache_dir()),
            loader: None,
        },
    );
    embedder.init().await.unwrap();
    assert_eq!(embedder.dimensions().unwrap(), 384);

    let mut failures = Vec::new();
    let mut floor = (f64::INFINITY, String::new());
    for file in &files {
        let content = std::fs::read_to_string(fixture_dir.join(file)).unwrap();
        // Empty files embed a single space, as the TypeScript test did.
        let input = if content.trim().is_empty() {
            " ".to_string()
        } else {
            content
        };
        let live = embedder.embed(&input, TaskType::Document).await.unwrap();
        let reference: Vec<f64> = vectors[file]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let sim = cosine(&live, &reference);
        if sim < floor.0 {
            floor = (sim, file.clone());
        }
        if sim < COSINE_THRESHOLD {
            failures.push(format!("{file}: cos={sim:.8}"));
        }
    }
    eprintln!(
        "[v4-equivalence] drift floor: cos={:.6} on {}",
        floor.0, floor.1
    );
    assert!(
        failures.is_empty(),
        "below {COSINE_THRESHOLD}: {failures:#?}"
    );
}
