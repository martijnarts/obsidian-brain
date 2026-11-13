//! Ported from `test/pipeline/reindex-recovery.test.ts`. The SQL-error
//! cases spied on better-sqlite3 to inject errors; here a dropped table
//! raises a real one. The case of a non-SQL error that escapes every guard
//! has no way in without such a spy and is not ported.

use std::sync::Arc;

use obsidian_brain::embeddings::Embedder;
use obsidian_brain::pipeline::{IndexPipeline, bootstrap};
use obsidian_brain::store::Db;
use obsidian_brain::store::sync::get_sync_mtime;

use crate::support::{FakeEmbedder, LogCapture, memory_db, multi_section_note, write};

fn setup() -> (tempfile::TempDir, Db) {
    let db = memory_db();
    db.with(|c| bootstrap(c, &FakeEmbedder::instant())).unwrap();
    (tempfile::tempdir().unwrap(), db)
}

fn pipeline(db: &Db, e: FakeEmbedder) -> IndexPipeline {
    let embedder: Arc<dyn Embedder> = Arc::new(e);
    IndexPipeline::new(db.clone(), embedder)
}

#[tokio::test]
async fn a_failed_note_level_embed_records_a_note_row_and_still_syncs() {
    let (dir, db) = setup();
    write(dir.path(), "failing-note.md", &multi_section_note(2));
    write(dir.path(), "passing-note.md", &multi_section_note(2));
    let embedder = Arc::new(FakeEmbedder::note_embed_fail(
        "failing-note",
        "input length exceeds context length",
    ));
    let p = IndexPipeline::new(db.clone(), embedder.clone());
    let stats = p.index(dir.path()).await.unwrap();
    assert_eq!(stats.nodes_indexed, 2);

    let reason: String = db
        .with(|c| {
            c.query_row(
                "SELECT reason FROM failed_chunks WHERE chunk_id = 'failing-note.md#note'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert!(reason == "note-too-long" || reason == "note-embed-error");
    assert!(
        db.with(|c| get_sync_mtime(c, "failing-note.md"))
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn a_dead_note_level_embed_aborts() {
    let (dir, db) = setup();
    write(dir.path(), "dead-note.md", &multi_section_note(1));
    let err = pipeline(
        &db,
        FakeEmbedder::always_fail("connect ECONNREFUSED 127.0.0.1:11434"),
    )
    .index(dir.path())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("ECONNREFUSED"));
}

#[tokio::test]
async fn self_heal_clears_the_sync_row_of_a_note_without_vectors_and_a_later_pass_recovers() {
    let (dir, db) = setup();
    // An empty note goes through the title fallback, whose text starts with
    // the stem, so its only chunk fails too.
    write(dir.path(), "recover-note.md", "");
    let stats = pipeline(
        &db,
        FakeEmbedder::note_embed_fail("recover-note", "input length exceeds context length"),
    )
    .index(dir.path())
    .await
    .unwrap();
    assert_eq!(stats.notes_missing_embeddings, 1);
    assert!(
        db.with(|c| get_sync_mtime(c, "recover-note.md"))
            .unwrap()
            .is_none()
    );

    write(dir.path(), "recover-note.md", &multi_section_note(2));
    let stats = pipeline(&db, FakeEmbedder::instant())
        .index(dir.path())
        .await
        .unwrap();
    assert_eq!(stats.notes_missing_embeddings, 0);
    assert!(
        db.with(|c| get_sync_mtime(c, "recover-note.md"))
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn happy_path_misses_no_embeddings() {
    let (dir, db) = setup();
    write(dir.path(), "happy-note.md", &multi_section_note(2));
    let stats = pipeline(&db, FakeEmbedder::instant())
        .index(dir.path())
        .await
        .unwrap();
    assert_eq!(stats.notes_missing_embeddings, 0);
}

#[tokio::test]
async fn an_sql_error_is_rethrown_with_guidance() {
    let (dir, db) = setup();
    write(dir.path(), "sql-err-note.md", &multi_section_note(1));
    let p = pipeline(&db, FakeEmbedder::instant());
    // Load the capacity first, so the failure lands inside the pass.
    p.refresh_capacity().await.unwrap();
    db.with(|c| c.execute_batch("DROP TABLE sync")).unwrap();
    let logs = LogCapture::start();
    let err = p.index(dir.path()).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("likely schema drift or stale install"),
        "{err}"
    );
    let text = logs.text();
    assert!(
        text.lines()
            .any(|l| l.contains("schema drift") && l.contains("rm -rf")),
        "{text}"
    );
}
