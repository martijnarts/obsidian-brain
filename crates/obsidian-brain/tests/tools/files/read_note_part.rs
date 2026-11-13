//! Tests of the `read_note_part` tool.

use serde_json::json;

use crate::support::{INVALID, Vault, assert_contains};

const NOTE: &[&str] = &[
    "---",                // 1
    "title: Plan",        // 2
    "# not a heading",    // 3
    "---",                // 4
    "# Plan",             // 5
    "",                   // 6
    "Intro paragraph",    // 7
    "second line ^intro", // 8
    "",                   // 9
    "## Goals",           // 10
    "- ship it ^goal1",   // 11
    "- test it",          // 12
    "",                   // 13
    "```md",              // 14
    "# Fake heading",     // 15
    "code ^fake",         // 16
    "```",                // 17
    "",                   // 18
    "### Detail",         // 19
    "detail text",        // 20
    "",                   // 21
    "## Notes ##",        // 22
    "| a | b |",          // 23
    "| - | - |",          // 24
    "",                   // 25
    "^table",             // 26
    "",                   // 27
    "# Archive",          // 28
    "## Goals",           // 29
    "old goals",          // 30
    "",
];

fn harness() -> Vault {
    let h = Vault::new();
    h.write("Plan.md", NOTE.join("\n"));
    h.node("Plan.md", "Plan", "", json!({}));
    h
}

async fn part(h: &Vault, args: serde_json::Value) -> serde_json::Value {
    h.ok("read_note_part", args).await
}

async fn fail(h: &Vault, args: serde_json::Value) -> String {
    h.err("read_note_part", args).await
}

#[tokio::test]
async fn outline_lists_headings_skipping_frontmatter_and_code_fences() {
    let h = harness();
    let data = part(&h, json!({ "name": "Plan", "mode": "outline" })).await;
    assert_eq!(data["path"], "Plan.md");
    assert_eq!(
        data["headings"],
        json!([
            { "level": 1, "text": "Plan", "line": 5 },
            { "level": 2, "text": "Goals", "line": 10 },
            { "level": 3, "text": "Detail", "line": 19 },
            { "level": 2, "text": "Notes", "line": 22 },
            { "level": 1, "text": "Archive", "line": 28 },
            { "level": 2, "text": "Goals", "line": 29 },
        ])
    );
}

#[tokio::test]
async fn outline_is_empty_for_a_note_without_headings() {
    let h = harness();
    h.write("Plain.md", "just text\n#tag is not a heading\n");
    h.node("Plain.md", "Plain", "", json!({}));
    let data = part(&h, json!({ "name": "Plain", "mode": "outline" })).await;
    assert_eq!(data["headings"], json!([]));
}

#[tokio::test]
async fn heading_returns_the_section_down_to_the_next_peer() {
    let h = harness();
    let data = part(
        &h,
        json!({ "name": "Plan", "mode": "heading", "heading": "Notes" }),
    )
    .await;
    assert_eq!(
        data["heading"],
        json!({ "level": 2, "text": "Notes", "line": 22 })
    );
    assert_eq!(data["startLine"], 22);
    assert_eq!(data["endLine"], 27);
    assert_eq!(
        data["content"].as_str().unwrap().split('\n').next(),
        Some("## Notes ##")
    );
}

#[tokio::test]
async fn heading_includes_deeper_headings_and_matches_case_insensitively() {
    let h = harness();
    let data = part(
        &h,
        json!({ "name": "Plan", "mode": "heading", "heading": "Plan" }),
    )
    .await;
    assert_eq!(data["startLine"], 5);
    assert_eq!(data["endLine"], 27);
    assert!(data["content"].as_str().unwrap().contains("### Detail"));
    let data = part(
        &h,
        json!({ "name": "Plan", "mode": "heading", "heading": "detail" }),
    )
    .await;
    assert_eq!(data["content"], "### Detail\ndetail text\n");
}

#[tokio::test]
async fn heading_runs_to_the_end_when_no_closing_heading_follows() {
    let h = harness();
    let data = part(
        &h,
        json!({ "name": "Plan", "mode": "heading", "heading": "Archive" }),
    )
    .await;
    assert_eq!(data["endLine"], 30);
    assert_eq!(data["content"], "# Archive\n## Goals\nold goals");
}

#[tokio::test]
async fn heading_ambiguity_nesting_and_misses() {
    let h = harness();
    assert_contains(
        &fail(
            &h,
            json!({ "name": "Plan", "mode": "heading", "heading": "Goals" }),
        )
        .await,
        &["ambiguous", "line 10, line 29"],
    );
    let data = part(
        &h,
        json!({ "name": "Plan", "mode": "heading", "heading": "Archive > Goals" }),
    )
    .await;
    assert_eq!(data["startLine"], 29);
    assert_eq!(data["content"], "## Goals\nold goals");
    let deep = part(
        &h,
        json!({ "name": "Plan", "mode": "heading", "heading": "plan > goals > detail" }),
    )
    .await;
    assert_eq!(deep["startLine"], 19);
    assert_contains(
        &fail(
            &h,
            json!({ "name": "Plan", "mode": "heading", "heading": "Archive > Detail" }),
        )
        .await,
        &["Heading \"Detail\" not found under \"Archive\""],
    );
    assert_contains(
        &fail(
            &h,
            json!({ "name": "Plan", "mode": "heading", "heading": "Nope > Goals" }),
        )
        .await,
        &["Heading \"Nope\" not found under the note"],
    );
    assert_contains(
        &fail(
            &h,
            json!({ "name": "Plan", "mode": "heading", "heading": "Missing" }),
        )
        .await,
        &["Heading \"Missing\" not found"],
    );
    assert_contains(
        &fail(
            &h,
            json!({ "name": "Plan", "mode": "heading", "heading": "Fake heading" }),
        )
        .await,
        &["not found"],
    );
    assert_contains(
        &fail(&h, json!({ "name": "Plan", "mode": "heading" })).await,
        &["needs `heading`"],
    );
}

#[tokio::test]
async fn a_heading_text_that_holds_an_arrow_matches_as_a_whole_first() {
    let h = harness();
    h.write("Arrow.md", "# A > B\nbody\n");
    h.node("Arrow.md", "Arrow", "", json!({}));
    let data = part(
        &h,
        json!({ "name": "Arrow", "mode": "heading", "heading": "A > B" }),
    )
    .await;
    assert_eq!(data["content"], "# A > B\nbody");
}

#[tokio::test]
async fn lines_returns_an_inclusive_range_that_counts_the_frontmatter() {
    let h = harness();
    let data = part(
        &h,
        json!({ "name": "Plan", "mode": "lines", "startLine": 1, "endLine": 2 }),
    )
    .await;
    assert_eq!(
        data,
        json!({
            "path": "Plan.md",
            "startLine": 1,
            "endLine": 2,
            "totalLines": 30,
            "content": "---\ntitle: Plan",
        })
    );
    let data = part(
        &h,
        json!({ "name": "Plan", "mode": "lines", "startLine": 30 }),
    )
    .await;
    assert_eq!(data["content"], "old goals");
    let data = part(
        &h,
        json!({ "name": "Plan", "mode": "lines", "startLine": 30, "endLine": 999 }),
    )
    .await;
    assert_eq!(data["endLine"], 30);
}

#[tokio::test]
async fn lines_errors() {
    let h = harness();
    assert_contains(
        &fail(
            &h,
            json!({ "name": "Plan", "mode": "lines", "startLine": 40 }),
        )
        .await,
        &["past the end"],
    );
    assert_contains(
        &fail(
            &h,
            json!({ "name": "Plan", "mode": "lines", "startLine": 5, "endLine": 4 }),
        )
        .await,
        &["before startLine"],
    );
    assert_contains(
        &fail(&h, json!({ "name": "Plan", "mode": "lines" })).await,
        &["needs `startLine`"],
    );
    assert_contains(
        &fail(
            &h,
            json!({ "name": "Plan", "mode": "lines", "startLine": 0 }),
        )
        .await,
        &[INVALID],
    );
}

#[tokio::test]
async fn block_returns_the_paragraph_or_list_item_a_block_id_ends() {
    let h = harness();
    let data = part(
        &h,
        json!({ "name": "Plan", "mode": "block", "blockId": "intro" }),
    )
    .await;
    assert_eq!(
        data,
        json!({
            "path": "Plan.md",
            "blockId": "intro",
            "startLine": 7,
            "endLine": 8,
            "content": "Intro paragraph\nsecond line ^intro",
        })
    );
    let data = part(
        &h,
        json!({ "name": "Plan", "mode": "block", "blockId": "^goal1" }),
    )
    .await;
    assert_eq!(data["startLine"], 11);
    assert_eq!(data["endLine"], 11);
    assert_eq!(data["content"], "- ship it ^goal1");
    let data = part(
        &h,
        json!({ "name": "Plan", "mode": "block", "blockId": "table" }),
    )
    .await;
    assert_eq!(data["startLine"], 23);
    assert_eq!(data["endLine"], 26);
    assert_eq!(data["content"], "| a | b |\n| - | - |\n\n^table");
}

#[tokio::test]
async fn block_errors() {
    let h = harness();
    assert_contains(
        &fail(
            &h,
            json!({ "name": "Plan", "mode": "block", "blockId": "fake" }),
        )
        .await,
        &["Block \"^fake\" not found"],
    );
    assert_contains(
        &fail(
            &h,
            json!({ "name": "Plan", "mode": "block", "blockId": "^^" }),
        )
        .await,
        &["blockId is empty"],
    );
    assert_contains(
        &fail(&h, json!({ "name": "Plan", "mode": "block" })).await,
        &["needs `blockId`"],
    );
}

#[tokio::test]
async fn reads_crlf_notes_with_the_same_line_numbers() {
    let h = harness();
    h.write("Crlf.md", "# One\r\ntext ^b1\r\n# Two\r\n");
    h.node("Crlf.md", "Crlf", "", json!({}));
    let data = part(&h, json!({ "name": "Crlf", "mode": "outline" })).await;
    assert_eq!(
        data["headings"],
        json!([
            { "level": 1, "text": "One", "line": 1 },
            { "level": 1, "text": "Two", "line": 3 },
        ])
    );
    let data = part(
        &h,
        json!({ "name": "Crlf", "mode": "block", "blockId": "b1" }),
    )
    .await;
    assert_eq!(data["content"], "text ^b1");
}

#[tokio::test]
async fn errors_on_a_missing_note_an_ambiguous_name_and_a_bad_mode() {
    let h = harness();
    h.node("a/Twin.md", "Twin A", "", json!({}));
    h.node("b/Twin.md", "Twin B", "", json!({}));
    assert_contains(
        &fail(&h, json!({ "name": "Nope", "mode": "outline" })).await,
        &["No note found"],
    );
    assert_contains(
        &fail(&h, json!({ "name": "twin", "mode": "outline" })).await,
        &["Multiple notes match"],
    );
    assert_contains(
        &fail(&h, json!({ "name": "Plan", "mode": "everything" })).await,
        &[INVALID],
    );
}

#[tokio::test]
async fn errors_when_the_indexed_note_is_gone_from_disk() {
    let h = harness();
    h.node("Ghost.md", "Ghost", "", json!({}));
    let e = fail(&h, json!({ "name": "Ghost", "mode": "outline" })).await;
    assert_eq!(
        e,
        format!(
            "ENOENT: no such file or directory, open '{}'",
            h.abs("Ghost.md").display()
        )
    );
}
