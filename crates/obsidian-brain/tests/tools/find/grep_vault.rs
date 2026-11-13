use serde_json::json;

use crate::support::{Fixture, fixture, write};

fn vault() -> Fixture {
    fixture(&[
        (
            "Alpha.md",
            "---\nstatus: draft\n---\n# Alpha\nfirst line\nthe Widget is here\nlast line",
        ),
        (
            "Projects/Beta.md",
            "widget one\r\nwidget two\r\nwidget three",
        ),
        ("Projects/Deep/Gamma.md", "no match here\n"),
        (".obsidian/workspace.md", "widget in config\n"),
        ("attachments/hidden.md", "widget in attachments\n"),
        ("notes.txt", "widget in a non-note\n"),
    ])
}

fn file_paths(out: &serde_json::Value) -> Vec<String> {
    out["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn finds_literal_matches_with_line_numbers_and_one_line_of_context() {
    let f = vault();
    let out = f.ok("grep_vault", json!({"query": "widget"})).await;
    assert_eq!(out["totalFiles"], 3);
    assert_eq!(out["scannedFiles"], 3);
    assert!(out.get("truncated").is_none());
    assert_eq!(file_paths(&out), ["Alpha.md", "Projects/Beta.md"]);
    assert_eq!(
        out["files"][0]["matches"],
        json!([{ "line": 6, "text": "the Widget is here", "before": ["first line"], "after": ["last line"] }])
    );
    // CRLF files split cleanly, and context stops at the file edges.
    assert_eq!(
        out["files"][1]["matches"][0],
        json!({ "line": 1, "text": "widget one", "before": [], "after": ["widget two"] })
    );
    assert_eq!(
        out["files"][1]["matches"][2],
        json!({ "line": 3, "text": "widget three", "before": ["widget two"], "after": [] })
    );
}

#[tokio::test]
async fn searches_frontmatter_too() {
    let f = vault();
    let out = f
        .ok(
            "grep_vault",
            json!({"query": "status: draft", "contextLines": 0}),
        )
        .await;
    assert_eq!(
        out["files"][0],
        json!({ "path": "Alpha.md", "matches": [{ "line": 2, "text": "status: draft" }] })
    );
}

#[tokio::test]
async fn case_sensitive_narrows_the_match() {
    let f = vault();
    let out = f
        .ok(
            "grep_vault",
            json!({"query": "Widget", "caseSensitive": true}),
        )
        .await;
    assert_eq!(file_paths(&out), ["Alpha.md"]);
}

#[tokio::test]
async fn regex_mode_matches_a_pattern_and_a_literal_query_does_not() {
    let f = vault();
    let re = f
        .ok(
            "grep_vault",
            json!({"query": "^widget (one|three)$", "regex": true, "contextLines": 0}),
        )
        .await;
    let lines: Vec<u64> = re["files"][0]["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["line"].as_u64().unwrap())
        .collect();
    assert_eq!(lines, [1, 3]);
    let lit = f
        .ok("grep_vault", json!({"query": "widget (one|three)"}))
        .await;
    assert_eq!(lit["files"], json!([]));
}

#[tokio::test]
async fn context_lines_widens_the_window() {
    let f = vault();
    let out = f
        .ok(
            "grep_vault",
            json!({"query": "widget is", "contextLines": 2}),
        )
        .await;
    assert_eq!(
        out["files"][0]["matches"][0]["before"],
        json!(["# Alpha", "first line"])
    );
}

#[tokio::test]
async fn max_matches_per_file_caps_lines_and_flags_the_rest() {
    let f = vault();
    let capped = f
        .ok(
            "grep_vault",
            json!({"query": "widget", "folder": "Projects", "maxMatchesPerFile": 2}),
        )
        .await;
    assert_eq!(capped["files"][0]["matches"].as_array().unwrap().len(), 2);
    assert_eq!(capped["files"][0]["moreMatches"], true);
    let exact = f
        .ok(
            "grep_vault",
            json!({"query": "widget", "folder": "Projects", "maxMatchesPerFile": 3}),
        )
        .await;
    assert_eq!(exact["files"][0]["matches"].as_array().unwrap().len(), 3);
    assert!(exact["files"][0].get("moreMatches").is_none());
}

#[tokio::test]
async fn limit_stops_at_that_many_files_and_reports_truncation() {
    let f = vault();
    let out = f
        .ok("grep_vault", json!({"query": "widget", "limit": 1}))
        .await;
    assert_eq!(out["files"].as_array().unwrap().len(), 1);
    assert_eq!(out["truncated"], true);
    assert_eq!(out["stoppedBy"], "limit");
    assert!(out["scannedFiles"].as_u64() < out["totalFiles"].as_u64());
}

#[tokio::test]
async fn folder_restricts_the_scan_recursively() {
    let f = vault();
    let out = f
        .ok(
            "grep_vault",
            json!({"query": "match", "folder": "Projects"}),
        )
        .await;
    assert_eq!(out["totalFiles"], 2);
    assert_eq!(file_paths(&out), ["Projects/Deep/Gamma.md"]);
}

#[tokio::test]
async fn clips_very_long_lines_around_the_match() {
    let f = vault();
    let long = format!("{}NEEDLE{}\nshort\n", "x".repeat(2000), "y".repeat(2000));
    f.write(&[("Long.md", &long)]);
    let out = f.ok("grep_vault", json!({"query": "needle"})).await;
    let file = out["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "Long.md")
        .unwrap();
    let text = file["matches"][0]["text"].as_str().unwrap();
    assert!(text.contains("NEEDLE"));
    assert!(text.encode_utf16().count() <= 402);
    assert!(text.starts_with('…') && text.ends_with('…'));
}

#[tokio::test]
async fn rejects_unsafe_and_invalid_regexes() {
    let f = vault();
    let e = f
        .err("grep_vault", json!({"query": "(a+)+", "regex": true}))
        .await;
    assert!(e.contains("quantified group"), "{e}");
    let e = f
        .err("grep_vault", json!({"query": "[", "regex": true}))
        .await;
    assert!(e.contains("Invalid regex"), "{e}");
}

#[tokio::test]
async fn refuses_folders_outside_the_vault_including_through_a_symlink() {
    let f = vault();
    let outside = tempfile::tempdir().unwrap();
    write(outside.path(), &[("secret.md", "widget secret\n")]);
    std::os::unix::fs::symlink(outside.path(), f.vault().join("Escape")).unwrap();
    for (folder, want) in [
        ("Escape", "outside the vault"),
        ("../x", "outside the vault"),
        ("C:/etc", "vault-relative"),
    ] {
        let e = f
            .err("grep_vault", json!({"query": "widget", "folder": folder}))
            .await;
        assert!(e.contains(want), "{folder}: {e}");
    }
}

#[tokio::test]
async fn an_unknown_folder_or_a_file_as_folder_is_an_error() {
    let f = vault();
    for folder in ["Nope", "Alpha.md"] {
        let e = f
            .err("grep_vault", json!({"query": "w", "folder": folder}))
            .await;
        assert!(e.contains("Folder not found"), "{e}");
    }
}

#[tokio::test]
async fn validation_bounds_the_numbers_and_requires_a_query() {
    let f = vault();
    assert!(!f.accepts("grep_vault", json!({"query": ""})).await);
    assert!(
        !f.accepts("grep_vault", json!({"query": "a", "contextLines": 11}))
            .await
    );
    assert!(
        !f.accepts("grep_vault", json!({"query": "a", "limit": 0}))
            .await
    );
    assert!(
        !f.accepts(
            "grep_vault",
            json!({"query": "a", "maxMatchesPerFile": 101})
        )
        .await
    );
    assert!(
        f.accepts(
            "grep_vault",
            json!({"query": "a", "regex": true, "caseSensitive": true, "contextLines": 0})
        )
        .await
    );
}
