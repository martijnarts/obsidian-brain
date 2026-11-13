//! Ported from `test/pipeline/empty-note-fallback.test.ts`.

use std::sync::Arc;

use obsidian_brain::embeddings::Embedder;
use obsidian_brain::pipeline::{IndexPipeline, bootstrap};
use obsidian_brain::store::Db;

use crate::support::{FakeEmbedder, memory_db, write};

fn setup() -> (tempfile::TempDir, Db, IndexPipeline) {
    let db = memory_db();
    db.with(|c| bootstrap(c, &FakeEmbedder::instant())).unwrap();
    let embedder: Arc<dyn Embedder> = Arc::new(FakeEmbedder::instant());
    let p = IndexPipeline::new(db.clone(), embedder);
    (tempfile::tempdir().unwrap(), db, p)
}

fn vec_rows(db: &Db, node_id: &str) -> i64 {
    db.with(|c| {
        c.query_row(
            "SELECT COUNT(*) FROM chunks c JOIN chunks_vec v ON c.rowid = v.rowid WHERE c.node_id = ?",
            [node_id],
            |r| r.get(0),
        )
    })
    .unwrap()
}

fn chunk_content(db: &Db, node_id: &str) -> String {
    db.with(|c| {
        c.query_row(
            "SELECT content FROM chunks WHERE node_id = ?",
            [node_id],
            |r| r.get(0),
        )
    })
    .unwrap()
}

#[tokio::test]
async fn an_empty_note_gets_a_fallback_chunk_from_its_stem() {
    let (dir, db, p) = setup();
    write(dir.path(), "just-a-stub.md", "");
    let stats = p.index(dir.path()).await.unwrap();
    assert_eq!(stats.nodes_indexed, 1);
    assert_eq!(vec_rows(&db, "just-a-stub.md"), 1);
    assert_eq!(stats.notes_missing_embeddings, 0);
    assert!(chunk_content(&db, "just-a-stub.md").contains("just-a-stub"));
}

#[tokio::test]
async fn a_frontmatter_only_note_gets_its_scalars() {
    let (dir, db, p) = setup();
    write(
        dir.path(),
        "meta.md",
        "---\nstatus: in-progress\nyear: 2026\n---\n",
    );
    p.index(dir.path()).await.unwrap();
    assert_eq!(vec_rows(&db, "meta.md"), 1);
    let content = chunk_content(&db, "meta.md");
    assert!(content.contains("status: in-progress"));
    assert!(content.contains("year: 2026"));
}

#[tokio::test]
async fn a_daily_note_heading_only_is_indexed() {
    let (dir, db, p) = setup();
    write(dir.path(), "2026-04-25.md", "# 2026-04-25\n");
    p.index(dir.path()).await.unwrap();
    assert_eq!(vec_rows(&db, "2026-04-25.md"), 1);
}

#[tokio::test]
async fn an_embeds_only_note_lists_its_targets() {
    let (dir, db, p) = setup();
    write(
        dir.path(),
        "gallery.md",
        "![[image1.png]] ![[image2.png]] ![[image3.png]]\n",
    );
    p.index(dir.path()).await.unwrap();
    assert_eq!(vec_rows(&db, "gallery.md"), 1);
    let content = chunk_content(&db, "gallery.md");
    for img in ["image1.png", "image2.png", "image3.png"] {
        assert!(content.contains(img));
    }
}

#[tokio::test]
async fn many_empty_notes_all_get_fallback_chunks() {
    let (dir, db, p) = setup();
    for i in 0..10 {
        write(dir.path(), &format!("daily-{i}.md"), "");
    }
    let stats = p.index(dir.path()).await.unwrap();
    assert_eq!(stats.nodes_indexed, 10);
    assert_eq!(stats.notes_missing_embeddings, 0);
    let n: i64 = db
        .with(|c| {
            c.query_row(
                "SELECT COUNT(DISTINCT node_id) FROM chunks JOIN chunks_vec ON chunks.rowid = chunks_vec.rowid",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(n, 10);
}

#[tokio::test]
async fn real_content_replaces_the_fallback_chunk() {
    let (dir, db, p) = setup();
    write(dir.path(), "evolves.md", "");
    p.index(dir.path()).await.unwrap();
    assert_eq!(vec_rows(&db, "evolves.md"), 1);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    write(
        dir.path(),
        "evolves.md",
        "# Heading One\n\nThis paragraph has enough content to pass the minimum chunk character threshold.\n\n# Heading Two\n\nAnother paragraph that exceeds the minimum chunk character threshold of fifty.\n",
    );
    p.index(dir.path()).await.unwrap();
    assert!(vec_rows(&db, "evolves.md") >= 2);
}
