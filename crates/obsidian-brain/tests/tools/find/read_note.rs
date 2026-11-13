use obsidian_brain::store::edges::insert_edge;
use obsidian_brain::store::nodes::upsert_node;
use obsidian_brain::types::{ParsedEdge, ParsedNode};
use serde_json::{Value, json};

use crate::support::{Fixture, fixture};

fn has(list: &Value, key: &str, id: &str) -> bool {
    list.as_array().unwrap().iter().any(|e| e[key] == id)
}

fn edge(f: &Fixture, source: &str, target: &str, context: &str) {
    f.db()
        .with(|c| {
            insert_edge(
                c,
                &ParsedEdge {
                    source_id: source.into(),
                    target_id: target.into(),
                    context: context.into(),
                    target_subpath: None,
                },
            )
        })
        .unwrap();
}

#[tokio::test]
async fn full_mode_reports_edges_with_their_context() {
    let f = fixture(&[]);
    f.upsert("a.md", "Alpha", json!({}));
    f.upsert("b.md", "Beta", json!({}));
    edge(&f, "a.md", "b.md", "link");
    edge(&f, "b.md", "a.md", "backlink");
    let out = f
        .ok("read_note", json!({"name": "a.md", "mode": "full"}))
        .await;
    assert_eq!(
        out["data"]["outgoing"],
        json!([{"targetId": "b.md", "targetTitle": "Beta", "context": "link"}])
    );
    assert_eq!(
        out["data"]["incoming"],
        json!([{"sourceId": "b.md", "sourceTitle": "Beta", "context": "backlink"}])
    );
    let keys: Vec<&String> = out["data"].as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "id",
            "title",
            "frontmatter",
            "content",
            "truncated",
            "outgoing",
            "incoming"
        ]
    );
}

#[tokio::test]
async fn brief_mode_lists_titles_and_hints_at_missing_notes() {
    let f = fixture(&[]);
    f.upsert("a.md", "Alpha", json!({"status": "x"}));
    f.upsert("b.md", "Beta", json!({}));
    edge(&f, "a.md", "b.md", "one");
    edge(&f, "a.md", "Ghost.md", "two");
    edge(&f, "a.md", "b.md", "three");
    edge(&f, "b.md", "a.md", "back");
    let out = f.ok("read_note", json!({"name": "Alpha"})).await;
    let data = &out["data"];
    let keys: Vec<&String> = data.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["id", "title", "frontmatter", "outgoing", "incoming"]);
    assert_eq!(data["frontmatter"], json!({"status": "x"}));
    assert_eq!(
        data["outgoing"],
        json!([
            {"targetId": "b.md", "targetTitle": "Beta"},
            {"targetId": "Ghost.md", "targetTitle": "Ghost.md"},
        ])
    );
    assert!(has(&data["incoming"], "sourceId", "b.md"));
    assert_eq!(out["context"]["next_actions"][0]["tool"], "create_note");
    assert_eq!(
        out["context"]["state"]["last_file_unresolved_links"],
        json!(["Ghost.md"])
    );

    let full = f
        .ok("read_note", json!({"name": "a.md", "mode": "full"}))
        .await;
    assert_eq!(full["data"]["outgoing"].as_array().unwrap().len(), 3);
    assert_eq!(full["data"]["outgoing"][1]["targetTitle"], "Ghost.md");
    assert_eq!(
        full["context"]["state"]["last_file_outgoing"],
        json!(["b.md", "Ghost.md"])
    );
}

#[tokio::test]
async fn full_mode_truncates_in_utf16_units() {
    let f = fixture(&[]);
    f.db()
        .with(|c| {
            upsert_node(
                c,
                &ParsedNode {
                    id: "e.md".into(),
                    title: "E".into(),
                    content: "😀😀x".into(),
                    frontmatter: Default::default(),
                },
            )
        })
        .unwrap();
    let out = f
        .ok(
            "read_note",
            json!({"name": "e.md", "mode": "full", "maxContentLength": 4}),
        )
        .await;
    assert_eq!(out["data"]["content"], "😀😀");
    assert_eq!(out["data"]["truncated"], true);
    let whole = f
        .ok(
            "read_note",
            json!({"name": "e.md", "mode": "full", "maxContentLength": 5}),
        )
        .await;
    assert_eq!(whole["data"]["truncated"], false);
}

#[tokio::test]
async fn an_unknown_note_is_an_error() {
    let f = fixture(&[]);
    assert_eq!(
        f.err("read_note", json!({"name": "nothing"})).await,
        "No note found matching \"nothing\""
    );
    assert!(
        !f.accepts("read_note", json!({"name": "x", "maxContentLength": 0}))
            .await
    );
}
