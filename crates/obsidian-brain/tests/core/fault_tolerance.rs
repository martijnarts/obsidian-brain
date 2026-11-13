//! Ported from `test/pipeline/indexer-fault-tolerance.test.ts`.

use std::sync::Arc;

use obsidian_brain::embeddings::Embedder;
use obsidian_brain::pipeline::{IndexPipeline, IndexStats, bootstrap};

use crate::support::{FakeEmbedder, LogCapture, memory_db, multi_section_note, write};

async fn index_with(
    embedder: FakeEmbedder,
    note: &str,
    sections: usize,
) -> anyhow::Result<IndexStats> {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), note, &multi_section_note(sections));
    let db = memory_db();
    db.with(|c| bootstrap(c, &FakeEmbedder::instant())).unwrap();
    let embedder: Arc<dyn Embedder> = Arc::new(embedder);
    IndexPipeline::new(db, embedder).index(dir.path()).await
}

#[tokio::test]
async fn happy_path_skips_no_chunks() {
    let stats = index_with(FakeEmbedder::instant(), "note.md", 2)
        .await
        .unwrap();
    assert_eq!(stats.chunks_skipped, 0);
    assert!(stats.chunks_ok > 0);
    assert!(stats.nodes_indexed > 0);
}

#[tokio::test]
async fn too_long_families_skip_the_chunk_and_index_the_note() {
    let cases = [
        ("input length exceeds the context length of 512", 2),
        ("This exceeds the maximum context length for this model", 2),
        ("Cannot broadcast [1, 512] to [1, 256]", 1),
        ("too many tokens in the sequence (1024 > 512)", 1),
        ("input_too_long: sequence exceeds max length", 1),
        ("shape mismatch: expected [1, 384] got [1, 512]", 1),
        ("context length exceeded", 1),
        // "neural network" is not a dead embedder.
        (
            "ONNX Runtime: neural network input tensor shape mismatch: expected [1,512] got [1,768]",
            1,
        ),
        // Unknown errors are skipped too: better than halting.
        ("some totally unexpected embedder error XYZ", 1),
    ];
    for (msg, sections) in cases {
        let stats = index_with(FakeEmbedder::faulty_on_call(0, msg), "note.md", sections)
            .await
            .unwrap();
        assert_eq!(stats.chunks_skipped, 1, "{msg}");
        assert_eq!(stats.nodes_indexed, 1, "{msg}");
    }
}

#[tokio::test]
async fn a_dead_embedder_aborts_the_pass() {
    for msg in [
        "connect ECONNREFUSED 127.0.0.1:11434",
        "getaddrinfo ENOTFOUND ollama.local",
    ] {
        let err = index_with(FakeEmbedder::always_fail(msg), "dead-note.md", 1)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains(msg.split(' ').nth(1).unwrap()),
            "{err}"
        );
    }
}

#[tokio::test]
async fn logs_a_summary_only_when_chunks_were_skipped() {
    let logs = LogCapture::start();
    index_with(
        FakeEmbedder::faulty_on_call(0, "input length exceeds the context length"),
        "skip-line-note.md",
        1,
    )
    .await
    .unwrap();
    let text = logs.text();
    let summary =
        regex::Regex::new(r"indexed \d+ notes \(\d+ chunks ok, \d+ chunks skipped\)\.").unwrap();
    assert!(summary.is_match(&text), "{text}");
    let skip = text
        .lines()
        .find(|l| l.contains("chunk too large for embedder") && l.contains("skipping"))
        .expect("a skip line");
    assert!(skip.contains("node: skip-line-note.md"));
    assert!(regex::Regex::new(r"chunk: \d+").unwrap().is_match(skip));
    assert!(regex::Regex::new(r"chars: \d+").unwrap().is_match(skip));
    drop(logs);

    let quiet = LogCapture::start();
    index_with(FakeEmbedder::instant(), "quiet-note.md", 1)
        .await
        .unwrap();
    assert!(!quiet.text().contains("chunks skipped"));
}

#[tokio::test]
async fn logs_an_unrecognised_error_skip() {
    let logs = LogCapture::start();
    index_with(
        FakeEmbedder::faulty_on_call(0, "some totally unexpected embedder error XYZ"),
        "unrec-note.md",
        1,
    )
    .await
    .unwrap();
    assert!(
        logs.text()
            .lines()
            .any(|l| l.contains("unrecognised error") && l.contains("skipping"))
    );
}
