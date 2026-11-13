//! Ported from `test/integration.test.ts`: the whole pipeline over the
//! fixture vault, with a bag-of-words fake embedder in place of the model.

use std::sync::Arc;

use obsidian_brain::embeddings::Embedder;
use obsidian_brain::graph::builder::KnowledgeGraph;
use obsidian_brain::graph::centrality::{betweenness_centrality_top, page_rank};
use obsidian_brain::graph::pathfinding::{
    common_neighbors, extract_subgraph, find_neighbors, find_paths,
};
use obsidian_brain::pipeline::{IndexPipeline, bootstrap};
use obsidian_brain::resolve::name_match::resolve_node_name;
use obsidian_brain::search::Search;
use obsidian_brain::store::edges::get_edges_by_source;
use obsidian_brain::store::nodes::get_node;

use crate::support::{FakeEmbedder, fixture_vault, memory_db};

#[tokio::test]
async fn full_pipeline_over_the_fixture_vault() {
    let db = memory_db();
    let embedder: Arc<dyn Embedder> = Arc::new(FakeEmbedder::bag_of_words());
    db.with(|c| bootstrap(c, &*embedder)).unwrap();
    IndexPipeline::new(db.clone(), embedder.clone())
        .index(&fixture_vault())
        .await
        .unwrap();
    let kg = db.with(|c| KnowledgeGraph::from_store(c, true)).unwrap();
    let search = Search::new(db.clone(), embedder);

    let matches = db.with(|c| resolve_node_name("A. Smith", c)).unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].node_id, "People/Alice Smith.md");

    let alice = db
        .with(|c| get_node(c, "People/Alice Smith.md"))
        .unwrap()
        .unwrap();
    assert_eq!(alice.title, "Alice Smith");
    assert!(
        !db.with(|c| get_edges_by_source(c, "People/Alice Smith.md"))
            .unwrap()
            .is_empty()
    );

    let neighbors = find_neighbors(&kg, "People/Alice Smith.md", 1);
    assert!(neighbors.iter().any(|n| n.title == "Widget Theory"));

    assert!(
        !search
            .semantic("design pattern for components", 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !search
            .fulltext("resilient components", 20)
            .unwrap()
            .is_empty()
    );

    assert!(!find_paths(&kg, "People/Alice Smith.md", "Ideas/Acme Project.md", 3).is_empty());
    let common = common_neighbors(&kg, "People/Alice Smith.md", "People/Bob Jones.md");
    assert!(common.iter().any(|n| n.title == "Widget Theory"));

    let sub = extract_subgraph(&kg, "Concepts/Widget Theory.md", 1, None).unwrap();
    assert!(sub.nodes.len() > 1);
    assert!(!sub.edges.is_empty());

    assert!(!betweenness_centrality_top(&kg.to_undirected(), 10).is_empty());
    let scores = page_rank(&kg.to_undirected()).unwrap();
    let alice_score = scores
        .iter()
        .find(|(id, _)| id == "People/Alice Smith.md")
        .unwrap()
        .1;
    assert!(alice_score > 0.0);

    assert!(db.with(|c| get_node(c, "orphan.md")).unwrap().is_some());
    assert!(find_neighbors(&kg, "orphan.md", 1).is_empty());
}
