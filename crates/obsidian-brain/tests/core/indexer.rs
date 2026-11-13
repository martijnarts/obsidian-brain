//! Tests of the indexer, with a fake embedder in place of the real model.

use std::sync::Arc;

use obsidian_brain::embeddings::Embedder;
use obsidian_brain::pipeline::{IndexPipeline, NoteEvent, bootstrap};
use obsidian_brain::store::Db;
use obsidian_brain::store::edges::get_edges_by_source;
use obsidian_brain::store::nodes::get_node;

use crate::support::{FakeEmbedder, fixture_vault, memory_db, write};

fn pipeline() -> (Db, IndexPipeline) {
    let db = memory_db();
    let embedder: Arc<dyn Embedder> = Arc::new(FakeEmbedder::instant());
    db.with(|c| bootstrap(c, &*embedder)).unwrap();
    let p = IndexPipeline::new(db.clone(), embedder);
    (db, p)
}

#[tokio::test]
async fn indexes_the_fixture_vault_with_stubs_for_broken_links() {
    let (db, p) = pipeline();
    let stats = p.index(&fixture_vault()).await.unwrap();
    assert!(stats.nodes_indexed > 0);
    assert!(stats.edges_indexed > 0);

    let alice = db
        .with(|c| get_node(c, "People/Alice Smith.md"))
        .unwrap()
        .unwrap();
    assert_eq!(alice.title, "Alice Smith");
    let edges = db
        .with(|c| get_edges_by_source(c, "People/Alice Smith.md"))
        .unwrap();
    assert!(!edges.is_empty());

    let acme = db
        .with(|c| get_edges_by_source(c, "Ideas/Acme Project.md"))
        .unwrap();
    assert!(acme.iter().any(|e| e.target_id.contains("Nonexistent")));
}

#[tokio::test]
async fn is_incremental() {
    let (_db, p) = pipeline();
    let first = p.index(&fixture_vault()).await.unwrap();
    assert!(first.nodes_indexed > 0);
    let second = p.index(&fixture_vault()).await.unwrap();
    assert_eq!(second.nodes_indexed, 0);
    assert_eq!(second.nodes_skipped, first.nodes_indexed);
}

#[tokio::test]
async fn resolves_forward_reference_stubs_when_the_real_note_appears() {
    let dir = tempfile::tempdir().unwrap();
    let (db, p) = pipeline();
    write(dir.path(), "_src.md", "# Src\n\nSee [[_future]].\n");
    p.index(dir.path()).await.unwrap();
    assert!(
        db.with(|c| get_node(c, "_stub/_future.md"))
            .unwrap()
            .is_some()
    );
    let before = db.with(|c| get_edges_by_source(c, "_src.md")).unwrap();
    assert!(before.iter().any(|e| e.target_id == "_stub/_future.md"));

    write(dir.path(), "_future.md", "# Future\n\nNow I exist.\n");
    p.index(dir.path()).await.unwrap();
    let after = db.with(|c| get_edges_by_source(c, "_src.md")).unwrap();
    assert!(after.iter().any(|e| e.target_id == "_future.md"));
    assert!(!after.iter().any(|e| e.target_id == "_stub/_future.md"));
    assert!(
        db.with(|c| get_node(c, "_stub/_future.md"))
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn index_single_note_adds_updates_and_deletes() {
    let dir = tempfile::tempdir().unwrap();
    let (db, p) = pipeline();
    let vault = dir.path();

    write(
        vault,
        "one.md",
        "# One\n\nFirst note with a [[two]] link.\n",
    );
    let r = p
        .index_single_note(vault, "one.md", NoteEvent::Add)
        .await
        .unwrap();
    assert!(r.indexed);
    assert!(db.with(|c| get_node(c, "one.md")).unwrap().is_some());
    assert_eq!(
        db.with(|c| get_edges_by_source(c, "one.md")).unwrap().len(),
        1
    );

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    write(
        vault,
        "one.md",
        "# One updated\n\nNo longer links anywhere.\n",
    );
    let r = p
        .index_single_note(vault, "one.md", NoteEvent::Change)
        .await
        .unwrap();
    assert!(r.indexed);
    assert_eq!(
        db.with(|c| get_node(c, "one.md")).unwrap().unwrap().title,
        "one"
    );
    assert!(
        db.with(|c| get_edges_by_source(c, "one.md"))
            .unwrap()
            .is_empty()
    );

    let r = p
        .index_single_note(vault, "one.md", NoteEvent::Unlink)
        .await
        .unwrap();
    assert!(r.deleted);
    assert!(db.with(|c| get_node(c, "one.md")).unwrap().is_none());
}

#[tokio::test]
async fn index_single_note_skips_when_the_mtime_has_not_advanced() {
    let dir = tempfile::tempdir().unwrap();
    let (_db, p) = pipeline();
    write(dir.path(), "stable.md", "# Stable\n");
    let first = p
        .index_single_note(dir.path(), "stable.md", NoteEvent::Add)
        .await
        .unwrap();
    assert!(first.indexed);
    let second = p
        .index_single_note(dir.path(), "stable.md", NoteEvent::Change)
        .await
        .unwrap();
    assert!(second.skipped);
    assert!(!second.indexed);
}

#[tokio::test]
async fn index_single_note_migrates_a_forward_stub() {
    let dir = tempfile::tempdir().unwrap();
    let (db, p) = pipeline();
    write(dir.path(), "Cars.md", "# Cars\n\nI drive a [[BMW]].\n");
    p.index_single_note(dir.path(), "Cars.md", NoteEvent::Add)
        .await
        .unwrap();
    assert!(db.with(|c| get_node(c, "_stub/BMW.md")).unwrap().is_some());
    let before = db.with(|c| get_edges_by_source(c, "Cars.md")).unwrap();
    assert!(before.iter().any(|e| e.target_id == "_stub/BMW.md"));

    write(dir.path(), "BMW.md", "# BMW\n\nReal note.\n");
    p.index_single_note(dir.path(), "BMW.md", NoteEvent::Add)
        .await
        .unwrap();
    assert!(db.with(|c| get_node(c, "_stub/BMW.md")).unwrap().is_none());
    let after = db.with(|c| get_edges_by_source(c, "Cars.md")).unwrap();
    assert!(after.iter().any(|e| e.target_id == "BMW.md"));
    assert!(!after.iter().any(|e| e.target_id == "_stub/BMW.md"));
}

#[tokio::test]
async fn heading_and_block_anchor_stub_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let (db, p) = pipeline();
    let vault = dir.path();

    write(
        vault,
        "Cars.md",
        "# Cars\n\nI drive a [[BMW#Specs]] model.\n",
    );
    p.index(vault).await.unwrap();
    assert!(db.with(|c| get_node(c, "_stub/BMW.md")).unwrap().is_some());
    assert!(
        db.with(|c| get_node(c, "_stub/BMW#Specs.md"))
            .unwrap()
            .is_none()
    );
    let edges = db.with(|c| get_edges_by_source(c, "Cars.md")).unwrap();
    let edge = edges
        .iter()
        .find(|e| e.target_id == "_stub/BMW.md")
        .unwrap();
    assert_eq!(edge.target_subpath.as_deref(), Some("Specs"));

    write(vault, "BMW.md", "# BMW\n\nReal note.\n");
    p.index(vault).await.unwrap();
    assert!(db.with(|c| get_node(c, "_stub/BMW.md")).unwrap().is_none());
    let edges = db.with(|c| get_edges_by_source(c, "Cars.md")).unwrap();
    let edge = edges.iter().find(|e| e.target_id == "BMW.md").unwrap();
    assert_eq!(edge.target_subpath.as_deref(), Some("Specs"));

    write(
        vault,
        "Notes.md",
        "# Notes\n\nSee [[BMW^abc123]] for the block.\n",
    );
    p.index(vault).await.unwrap();
    let edges = db.with(|c| get_edges_by_source(c, "Notes.md")).unwrap();
    let edge = edges.iter().find(|e| e.target_id == "BMW.md").unwrap();
    assert_eq!(edge.target_subpath.as_deref(), Some("abc123"));
    assert!(
        db.with(|c| get_node(c, "_stub/BMW^abc123.md"))
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_deleted_file_leaves_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let (db, p) = pipeline();
    write(dir.path(), "gone.md", "# Gone\n\nSoon.\n");
    p.index(dir.path()).await.unwrap();
    assert!(db.with(|c| get_node(c, "gone.md")).unwrap().is_some());
    std::fs::remove_file(dir.path().join("gone.md")).unwrap();
    p.index(dir.path()).await.unwrap();
    assert!(db.with(|c| get_node(c, "gone.md")).unwrap().is_none());
}
