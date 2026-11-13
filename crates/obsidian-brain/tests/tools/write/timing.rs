//! Ported from `test/integration/server-init-timing/write-tools-*.test.ts`:
//! every write tool returns while the embedder is still loading, and its
//! reindex runs once the embedder is ready.

use std::sync::Arc;
use std::time::{Duration, Instant};

use obsidian_brain::store::nodes::get_node;
use serde_json::{Value, json};

use crate::support::{TestEmbedder, Vault};

/// A vault indexed with a ready embedder, then a fresh context over the
/// same files whose embedder is still loading.
async fn loading_vault(files: &[(&str, &str)]) -> (Vault, Arc<TestEmbedder>) {
    let embedder = Arc::new(TestEmbedder::gated());
    let v = Vault::with_embedder(embedder.clone());
    for (rel, content) in files {
        v.write(rel, content);
    }
    // Seed the index directly; the context's embedder stays gated.
    v.ctx.pipeline.index(v.path()).await.unwrap();
    (v, embedder)
}

async fn check(files: &[(&str, &str)], tool: &str, args: Value, indexed: &str) {
    let (v, embedder) = loading_vault(files).await;
    let start = Instant::now();
    v.ok(tool, args).await;
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "{tool} took {:?}",
        start.elapsed()
    );
    assert!(!v.ctx.embedder_ready());
    assert_eq!(v.ctx.background_reindex_count(), 1);

    embedder.release();
    tokio::time::timeout(Duration::from_secs(10), v.ctx.wait_for_background_reindex())
        .await
        .expect("the reindex finishes once the embedder is ready");
    assert!(v.ctx.embedder_ready());
    assert!(
        v.db().with(|c| get_node(c, indexed)).unwrap().is_some(),
        "{tool}: {indexed} was not indexed"
    );
}

#[tokio::test]
async fn create_note_returns_at_once_and_reindexes_later() {
    check(
        &[],
        "create_note",
        json!({ "title": "MyNote", "content": "hello world" }),
        "MyNote.md",
    )
    .await;
}

#[tokio::test]
async fn edit_note_returns_at_once_and_reindexes_later() {
    check(
        &[("EditLater.md", "# EditLater\n\nbody\n")],
        "edit_note",
        json!({ "name": "EditLater", "mode": "append", "content": "more text" }),
        "EditLater.md",
    )
    .await;
}

#[tokio::test]
async fn delete_note_returns_at_once_and_reindexes_later() {
    check(
        &[
            ("DeleteLater.md", "# DeleteLater\n\nbody\n"),
            ("Other.md", "x\n"),
        ],
        "delete_note",
        json!({ "name": "DeleteLater", "confirm": true }),
        "Other.md",
    )
    .await;
}

#[tokio::test]
async fn move_note_returns_at_once_and_reindexes_later() {
    check(
        &[("MoveLater.md", "# MoveLater\n\nbody\n")],
        "move_note",
        json!({ "source": "MoveLater", "destination": "MoveLaterRenamed" }),
        "MoveLaterRenamed.md",
    )
    .await;
}

#[tokio::test]
async fn link_notes_returns_at_once_and_reindexes_later() {
    check(
        &[
            ("LinkA.md", "# LinkA\n\nbody\n"),
            ("LinkB.md", "# LinkB\n\nbody\n"),
        ],
        "link_notes",
        json!({ "source": "LinkA", "target": "LinkB", "context": "background link test" }),
        "LinkA.md",
    )
    .await;
}
