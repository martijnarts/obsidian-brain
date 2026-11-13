//! Tests of the `file_info` tool.

use chrono::{DateTime, SecondsFormat};
use serde_json::json;

use crate::support::{Vault, assert_contains, outside, symlink};

const NOTE: &str = "---
tags: [project, \"#work\"]
---
# Title
Some #inline text with #project again.
## Tasks
- [ ] open one
- [x] done one
* [X] done two
1. [ ] numbered open
```
- [ ] not a task
# not a heading
```
";

fn harness() -> Vault {
    let h = Vault::new();
    h.write("Projects/Plan.md", NOTE);
    h.write("assets/pic.png", [1u8, 2, 3, 4]);
    h.node("Projects/Plan.md", "Plan", "", json!({}));
    h.node("Other.md", "Other", "", json!({}));
    h.edge("Projects/Plan.md", "Other.md", "");
    h.edge("Projects/Plan.md", "_stub/Missing.md", "");
    h.edge("Other.md", "Projects/Plan.md", "");
    h
}

fn is_iso(s: &str) -> bool {
    DateTime::parse_from_rfc3339(s)
        .is_ok_and(|d| d.to_utc().to_rfc3339_opts(SecondsFormat::Millis, true) == s)
}

#[tokio::test]
async fn describes_a_note() {
    let h = harness();
    let data = h
        .ok("file_info", json!({ "path": "Projects/Plan.md" }))
        .await;
    assert_eq!(data["path"], "Projects/Plan.md");
    assert_eq!(data["kind"], "note");
    assert_eq!(data["size"], NOTE.len());
    assert_eq!(data["indexed"], true);
    assert_eq!(
        data["links"],
        json!({ "outgoing": 2, "incoming": 1, "unresolved": 1 })
    );
    assert_eq!(data["headings"], 2);
    assert_eq!(data["tasks"], json!({ "open": 2, "done": 2 }));
    let mut tags: Vec<&str> = data["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap())
        .collect();
    tags.sort();
    assert_eq!(tags, ["inline", "project", "work"]);
    assert!(is_iso(data["mtime"].as_str().unwrap()), "{}", data["mtime"]);
    assert!(is_iso(data["ctime"].as_str().unwrap()), "{}", data["ctime"]);
    assert!(data.get("content").is_none());
}

#[tokio::test]
async fn reads_a_note_the_index_does_not_know_yet() {
    let h = harness();
    h.write("New.md", "tags: none\n#solo\n");
    let data = h.ok("file_info", json!({ "path": "New.md" })).await;
    assert_eq!(data["indexed"], false);
    assert_eq!(
        data["links"],
        json!({ "outgoing": 0, "incoming": 0, "unresolved": 0 })
    );
    assert_eq!(data["tags"], json!(["solo"]));
}

#[tokio::test]
async fn counts_tags_from_a_comma_separated_string_and_a_tag_key() {
    let h = harness();
    h.write("Str.md", "---\ntags: \"a, b c\"\ntag: d\n---\nbody\n");
    let data = h.ok("file_info", json!({ "path": "Str.md" })).await;
    assert_eq!(data["tags"], json!(["a", "b", "c", "d"]));
}

#[tokio::test]
async fn falls_back_to_the_whole_file_when_the_frontmatter_is_malformed() {
    let h = harness();
    h.write("Bad.md", "---\nkey: [unclosed\n---\n# H\n");
    let data = h.ok("file_info", json!({ "path": "Bad.md" })).await;
    assert_eq!(data["kind"], "note");
    assert_eq!(data["tags"], json!([]));
}

#[tokio::test]
async fn describes_an_attachment_with_its_size_and_no_note_fields() {
    let h = harness();
    let data = h.ok("file_info", json!({ "path": "assets/pic.png" })).await;
    assert_eq!(data["path"], "assets/pic.png");
    assert_eq!(data["kind"], "attachment");
    assert_eq!(data["size"], 4);
    assert!(data.get("links").is_none());
}

#[tokio::test]
async fn describes_a_folder_with_its_child_count_and_the_root_as_empty() {
    let h = harness();
    std::fs::create_dir(h.abs("Projects/Sub")).unwrap();
    let data = h.ok("file_info", json!({ "path": "Projects/" })).await;
    assert_eq!(data["path"], "Projects");
    assert_eq!(data["kind"], "folder");
    assert_eq!(data["children"], 2);
    let data = h.ok("file_info", json!({ "path": "" })).await;
    assert_eq!(data["path"], "");
    assert_eq!(data["kind"], "folder");
}

#[tokio::test]
async fn errors_on_a_missing_path() {
    let h = harness();
    assert_eq!(
        h.err("file_info", json!({ "path": "Nope.md" })).await,
        "Path not found: Nope.md"
    );
}

#[tokio::test]
async fn refuses_paths_that_escape_the_vault() {
    let h = harness();
    for path in ["../etc/passwd", "/etc/passwd"] {
        assert_contains(
            &h.err("file_info", json!({ "path": path })).await,
            &["outside the vault"],
        );
    }
    let out = outside();
    symlink(out.path(), &h.abs("link"));
    assert_contains(
        &h.err("file_info", json!({ "path": "link" })).await,
        &["outside the vault"],
    );
}
