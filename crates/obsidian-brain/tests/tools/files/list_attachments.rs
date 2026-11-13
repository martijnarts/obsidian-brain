//! Ported from `test/tools/list-attachments.test.ts`.

use serde_json::{Value, json};

use crate::support::{INVALID, Vault, assert_contains};

fn harness() -> Vault {
    let h = Vault::new();
    h.write("attachments/photo.png", [0u8; 10]);
    h.write("attachments/Scan.PDF", [0u8; 20]);
    h.write("docs/my file.pdf", [0u8; 5]);
    h.write("docs/local.jpg", [0u8; 1]);
    h.write("other/local.jpg", [0u8; 2]);
    h.write("orphan.zip", [0u8; 3]);
    h.write("Note.md", "not an attachment");
    h.write(".obsidian/workspace.json", "{}");
    h.write(".hidden/secret.png", "x");

    let note = [
        "![[photo.png]]",
        "[[photo.png|again]] counts once per note",
        "[pdf](docs/my%20file.pdf)",
        "![scan](<attachments/Scan.PDF> \"title\")",
        "[web](https://example.com/photo.png)",
        "`![[orphan.zip]]`",
        "```",
        "![[orphan.zip]]",
        "```",
    ]
    .join("\n");
    h.node("Note.md", "Note", &note, json!({}));
    h.node(
        "docs/Page.md",
        "Page",
        "![](local.jpg) and [[attachments/photo.png#frag]] and [x](./my%20file.pdf)",
        json!({}),
    );
    h.node(
        "other/Deep.md",
        "Deep",
        "![[other/local.jpg]] [[Missing.png]] [[]]",
        json!({}),
    );
    h.node(
        "_stub/orphan.zip.md",
        "orphan.zip",
        "![[orphan.zip]]",
        json!({ "_stub": true }),
    );
    h
}

fn paths(data: &Value) -> Vec<&str> {
    data["attachments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["path"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn lists_every_non_markdown_file_with_size_and_reference_count() {
    let h = harness();
    let data = h.ok("list_attachments", json!({})).await;
    assert_eq!(data["total"], 6);
    assert_eq!(
        data["attachments"],
        json!([
            { "path": "attachments/Scan.PDF", "extension": "pdf", "size": 20, "references": 1 },
            { "path": "attachments/photo.png", "extension": "png", "size": 10, "references": 2 },
            { "path": "docs/local.jpg", "extension": "jpg", "size": 1, "references": 1 },
            { "path": "docs/my file.pdf", "extension": "pdf", "size": 5, "references": 2 },
            { "path": "orphan.zip", "extension": "zip", "size": 3, "references": 0 },
            { "path": "other/local.jpg", "extension": "jpg", "size": 2, "references": 1 },
        ])
    );
}

#[tokio::test]
async fn unreferenced_only_keeps_the_orphans() {
    let h = harness();
    let data = h
        .ok("list_attachments", json!({ "unreferencedOnly": true }))
        .await;
    assert_eq!(paths(&data), ["orphan.zip"]);
}

#[tokio::test]
async fn filters_extensions_case_insensitively_with_or_without_the_dot() {
    let h = harness();
    let data = h
        .ok("list_attachments", json!({ "extensions": [".PDF", "zip"] }))
        .await;
    assert_eq!(
        paths(&data),
        ["attachments/Scan.PDF", "docs/my file.pdf", "orphan.zip"]
    );
}

#[tokio::test]
async fn scopes_to_a_folder_still_counting_references_from_anywhere() {
    let h = harness();
    let data = h
        .ok("list_attachments", json!({ "folder": "attachments/" }))
        .await;
    let got: Vec<(&str, u64)> = data["attachments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["path"].as_str().unwrap(),
                a["references"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [("attachments/Scan.PDF", 1), ("attachments/photo.png", 2)]
    );
}

#[tokio::test]
async fn pages_with_limit_and_offset() {
    let h = harness();
    let data = h
        .ok("list_attachments", json!({ "limit": 2, "offset": 1 }))
        .await;
    assert_eq!(data["total"], 6);
    assert_eq!(data["offset"], 1);
    assert_eq!(data["limit"], 2);
    assert_eq!(paths(&data), ["attachments/photo.png", "docs/local.jpg"]);
}

#[tokio::test]
async fn errors_on_a_missing_folder_a_file_as_folder_and_an_escape() {
    let h = harness();
    assert_eq!(
        h.err("list_attachments", json!({ "folder": "nope" })).await,
        "Folder not found: \"nope\""
    );
    assert_contains(
        &h.err("list_attachments", json!({ "folder": "orphan.zip" }))
            .await,
        &["Folder not found"],
    );
    assert_contains(
        &h.err("list_attachments", json!({ "folder": "../" })).await,
        &["outside the vault"],
    );
    assert_contains(
        &h.err("list_attachments", json!({ "limit": 0 })).await,
        &[INVALID],
    );
    assert_contains(
        &h.err("list_attachments", json!({ "offset": -1 })).await,
        &[INVALID],
    );
}
