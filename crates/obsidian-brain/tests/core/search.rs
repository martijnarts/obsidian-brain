//! Tests of unified search and Reciprocal Rank Fusion.
//! A bag-of-words fake embedder stands in for the real model.

use std::sync::Arc;

use obsidian_brain::embeddings::embedder::TransformersEmbedder;
use obsidian_brain::embeddings::{Embedder, TaskType};
use obsidian_brain::search::{Search, SearchUnique, reciprocal_rank_fusion};
use obsidian_brain::store::chunks::{upsert_chunk_row, upsert_chunk_vector};
use obsidian_brain::store::db::ensure_vec_tables;
use obsidian_brain::store::embeddings::upsert_embedding;
use obsidian_brain::store::nodes::upsert_node;
use obsidian_brain::types::{Chunk, ParsedNode};

use crate::support::{FakeEmbedder, memory_db};

async fn seeded() -> Search {
    let db = memory_db();
    let embedder: Arc<dyn Embedder> = Arc::new(FakeEmbedder::bag_of_words());
    db.with(|c| {
        c.execute_batch("DROP TABLE nodes_vec; DROP TABLE chunks_vec;")?;
        ensure_vec_tables(c, 1024).map_err(|e| anyhow::anyhow!(e))
    })
    .unwrap();
    let nodes = [
        (
            "graph.md",
            "Graph Theory",
            "Study of mathematical structures used to model pairwise relations",
        ),
        (
            "cake.md",
            "Chocolate Cake",
            "A delicious dessert made with cocoa powder and sugar",
        ),
        (
            "network.md",
            "Network Analysis",
            "Analysis of graph structures in social networks",
        ),
    ];
    for (id, title, content) in nodes {
        let node = ParsedNode {
            id: id.into(),
            title: title.into(),
            content: content.into(),
            frontmatter: Default::default(),
        };
        let text = TransformersEmbedder::build_embedding_text(title, &[], content);
        let note_vec = embedder.embed(&text, TaskType::Document).await.unwrap();
        let chunk = Chunk {
            chunk_index: 0,
            heading: Some(format!("{title} heading")),
            heading_level: Some(1),
            content: content.into(),
            content_hash: "hash-chunk-0".into(),
            start_line: 1,
            end_line: 8,
        };
        let chunk_vec = embedder.embed(content, TaskType::Document).await.unwrap();
        db.with(|c| {
            upsert_node(c, &node)?;
            upsert_embedding(c, id, &note_vec)?;
            let rowid = upsert_chunk_row(c, id, &chunk)?;
            upsert_chunk_vector(c, rowid, &chunk_vec)
        })
        .unwrap();
    }
    Search::new(db, embedder)
}

#[tokio::test]
async fn semantic_ranks_related_notes_first() {
    let search = seeded().await;
    let results = search
        .semantic("graph structures and relationships", 20)
        .await
        .unwrap();
    let ids: Vec<&str> = results.iter().map(|r| r.node_id.as_str()).collect();
    let graph = ids.iter().position(|i| *i == "graph.md").unwrap();
    let cake = ids.iter().position(|i| *i == "cake.md").unwrap();
    assert!(graph < cake, "{ids:?}");
    // One row per note, with the fields of its best chunk.
    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|r| r.chunk_id.is_some()));
}

#[tokio::test]
async fn fulltext_finds_keywords_and_nothing_for_unknown_words() {
    let search = seeded().await;
    let results = search.fulltext("cocoa powder", 20).unwrap();
    assert_eq!(results[0].node_id, "cake.md");
    assert!(
        search
            .fulltext("xyzzy_no_such_word", 20)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn hybrid_has_chunk_fields_only_with_chunks() {
    let search = seeded().await;
    let chunks = search
        .hybrid("graph structures", 5, SearchUnique::Chunks)
        .await
        .unwrap();
    assert!(!chunks.is_empty());
    for r in &chunks {
        assert!(r.chunk_id.is_some());
        assert!(r.chunk_heading.is_some());
    }
    let notes = search
        .hybrid("graph structures", 5, SearchUnique::Notes)
        .await
        .unwrap();
    assert!(!notes.is_empty());
    for r in &notes {
        assert!(r.chunk_id.is_none());
        let json = serde_json::to_value(r).unwrap();
        assert!(json.get("chunkId").is_none());
        assert_eq!(
            json.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["nodeId", "title", "score", "excerpt"]
        );
    }
}

#[tokio::test]
async fn semantic_falls_back_to_note_vectors_without_chunks() {
    // An index with note vectors and no chunks.
    let db = memory_db();
    let embedder: Arc<dyn Embedder> = Arc::new(FakeEmbedder::bag_of_words());
    db.with(|c| {
        c.execute_batch("DROP TABLE nodes_vec; DROP TABLE chunks_vec;")?;
        ensure_vec_tables(c, 1024).map_err(|e| anyhow::anyhow!(e))
    })
    .unwrap();
    let v = embedder.embed("cocoa", TaskType::Document).await.unwrap();
    db.with(|c| {
        upsert_node(
            c,
            &ParsedNode {
                id: "a.md".into(),
                title: "A".into(),
                content: "cocoa".into(),
                frontmatter: Default::default(),
            },
        )?;
        upsert_embedding(c, "a.md", &v)
    })
    .unwrap();
    let legacy = Search::new(db, embedder);
    let results = legacy.semantic("cocoa", 5).await.unwrap();
    assert_eq!(results[0].node_id, "a.md");
    assert!(results[0].chunk_id.is_none());
}

#[derive(Debug, PartialEq)]
struct Item(&'static str);

fn ids(fused: &[(&Item, f64)]) -> Vec<&'static str> {
    fused.iter().map(|(i, _)| i.0).collect()
}

#[test]
fn rrf_empty_and_single_list() {
    assert!(reciprocal_rank_fusion::<Item>(&[], |i| i.0.into(), 60.0).is_empty());
    assert!(reciprocal_rank_fusion::<Item>(&[vec![]], |i| i.0.into(), 60.0).is_empty());
    let list = vec![vec![Item("a"), Item("b"), Item("c")]];
    assert_eq!(
        ids(&reciprocal_rank_fusion(&list, |i| i.0.into(), 60.0)),
        ["a", "b", "c"]
    );
}

#[test]
fn rrf_adds_contributions_across_lists() {
    let lists = vec![
        vec![Item("shared"), Item("sem-only")],
        vec![Item("shared"), Item("ft-only")],
    ];
    let fused = reciprocal_rank_fusion(&lists, |i| i.0.into(), 60.0);
    assert_eq!(fused[0].0.0, "shared");
    assert!((fused[0].1 - 2.0 / 61.0).abs() < 1e-12);
}

#[test]
fn rrf_later_ranks_contribute_less_and_overlap_wins_over_equal_ranks() {
    let list = vec![vec![Item("top"), Item("mid"), Item("tail")]];
    let fused = reciprocal_rank_fusion(&list, |i| i.0.into(), 60.0);
    assert!(fused[0].1 > fused[2].1);

    let lists = vec![
        vec![Item("sem1"), Item("sem2"), Item("both")],
        vec![Item("ft1"), Item("ft2"), Item("both")],
    ];
    let order = ids(&reciprocal_rank_fusion(&lists, |i| i.0.into(), 60.0));
    let pos = |id| order.iter().position(|x| *x == id).unwrap();
    assert!(pos("both") < pos("sem2"));
    assert!(pos("both") < pos("ft2"));
}

#[test]
fn rrf_respects_k() {
    let list = vec![vec![Item("a")]];
    assert!((reciprocal_rank_fusion(&list, |i| i.0.into(), 0.0)[0].1 - 1.0).abs() < 1e-12);
    assert!(
        (reciprocal_rank_fusion(&list, |i| i.0.into(), 100.0)[0].1 - 1.0 / 101.0).abs() < 1e-12
    );
}
