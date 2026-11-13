//! Ported from `test/tools/vault-overview.test.ts`.

use obsidian_brain::store::sync::set_sync_mtime;
use serde_json::json;

use crate::support::Vault;

fn mtime(h: &Vault, id: &str, ms: f64) {
    h.ctx.db.with(|c| set_sync_mtime(c, id, ms, None)).unwrap();
}

#[tokio::test]
async fn returns_an_all_empty_snapshot_for_an_empty_vault() {
    let h = Vault::new();
    assert_eq!(
        h.ok("vault_overview", json!({})).await,
        json!({ "notes": 0, "attachments": 0, "folders": [], "topTags": [], "recent": [] })
    );
}

#[tokio::test]
async fn counts_notes_attachments_folders_tags_and_recent_notes() {
    let h = Vault::new();
    h.write("attachments/pic.png", "x");
    h.write("doc.pdf", "x");
    h.write("root.md", "# Root");
    h.write(".obsidian/app.json", "{}");

    h.node("root.md", "Root", "", json!({ "tags": ["x"] }));
    h.node("P/a.md", "A", "", json!({ "tags": ["x/y"] }));
    h.node("P/b.md", "B", "", json!({ "inline_tags": ["z"] }));
    h.node("Q/c.md", "C", "", json!({}));
    h.node("_stub/s.md", "S", "", json!({ "_stub": true }));
    mtime(&h, "root.md", 1_000.0);
    mtime(&h, "P/a.md", 3_000.0);
    mtime(&h, "P/b.md", 2_000.0);

    let out = h.ok("vault_overview", json!({})).await;
    assert_eq!(out["notes"], 4);
    assert_eq!(out["attachments"], 2);
    assert_eq!(
        out["folders"],
        json!([
            { "folder": "P", "count": 2 },
            { "folder": "(root)", "count": 1 },
            { "folder": "Q", "count": 1 },
        ])
    );
    assert_eq!(
        out["topTags"],
        json!([
            { "tag": "x", "count": 2 },
            { "tag": "x/y", "count": 1 },
            { "tag": "z", "count": 1 },
        ])
    );
    assert_eq!(
        out["recent"],
        json!([
            { "id": "P/a.md", "title": "A", "mtime": "1970-01-01T00:00:03.000Z" },
            { "id": "P/b.md", "title": "B", "mtime": "1970-01-01T00:00:02.000Z" },
            { "id": "root.md", "title": "Root", "mtime": "1970-01-01T00:00:01.000Z" },
        ])
    );
}

#[tokio::test]
async fn top_tags_and_recent_limit_the_snapshot() {
    let h = Vault::new();
    for i in 0..20 {
        let id = format!("n{i}.md");
        h.node(
            &id,
            &format!("n{i}"),
            "",
            json!({ "tags": [format!("t{i}")] }),
        );
        mtime(&h, &id, 1_000.0 + i as f64);
    }
    let out = h.ok("vault_overview", json!({})).await;
    assert_eq!(out["topTags"].as_array().unwrap().len(), 15);
    assert_eq!(out["recent"].as_array().unwrap().len(), 10);
    assert_eq!(out["recent"][0]["id"], "n19.md");

    let out = h
        .ok("vault_overview", json!({ "topTags": 3, "recent": 2 }))
        .await;
    assert_eq!(out["topTags"].as_array().unwrap().len(), 3);
    let ids: Vec<&str> = out["recent"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["n19.md", "n18.md"]);
}
