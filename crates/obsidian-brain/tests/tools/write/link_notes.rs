//! Tests of the `link_notes` tool.

use obsidian_brain::store::edges::get_edges_by_target;
use serde_json::json;

use crate::support::Vault;

#[tokio::test]
async fn a_link_adds_the_edge_and_the_line() {
    let v = Vault::new();
    v.write("A.md", "# A\n\nbody\n");
    v.write("B.md", "# B\n\nbody\n");
    v.index().await;
    let out = v
        .ok(
            "link_notes",
            json!({ "source": "A", "target": "B", "context": "relates to" }),
        )
        .await;
    assert_eq!(
        out,
        json!({ "source": "A.md", "target": "B", "context": "relates to" })
    );
    // The edge is there before any reindex runs.
    let inbound = v.db().with(|c| get_edges_by_target(c, "B.md")).unwrap();
    assert!(inbound.iter().any(|e| e.source_id == "A.md"));
    assert_eq!(v.read("A.md"), "# A\n\nbody\n\nrelates to [[B]]");
}

#[tokio::test]
async fn a_dry_run_returns_the_line_without_writing() {
    let v = Vault::new();
    v.write("A.md", "# A\n");
    v.row("A.md", "A");
    let out = v
        .ok(
            "link_notes",
            json!({ "source": "A", "target": "New idea", "context": "see", "dryRun": true }),
        )
        .await;
    assert_eq!(
        out,
        json!({
            "dryRun": true,
            "source": "A.md",
            "target": "New idea",
            "context": "see",
            "wouldAppend": "\nsee [[New idea]]",
        })
    );
    assert_eq!(v.read("A.md"), "# A\n");
    assert_eq!(v.ctx.background_reindex_count(), 0);
}

#[tokio::test]
async fn an_empty_context_is_refused() {
    let v = Vault::new();
    v.write("A.md", "# A\n");
    v.row("A.md", "A");
    let err = v
        .err(
            "link_notes",
            json!({ "source": "A", "target": "B", "context": "" }),
        )
        .await;
    assert!(err.contains("context"), "{err}");
    assert_eq!(v.read("A.md"), "# A\n");
}
