use serde_json::json;

use crate::support::{Fixture, fixture, paths};

fn vault() -> Fixture {
    let f = fixture(&[
        (
            "Finance/Kelly criterion.md",
            "# Kelly\n\nSee [[Missing note]].\n",
        ),
        (
            "Finance/Kelly notes.md",
            "---\ntitle: Betting sizes\n---\nbody\n",
        ),
        (
            "People/Bob Smith.md",
            "---\naliases: [Robert, Bobby Tables]\n---\nhi\n",
        ),
        ("People/Robert Jones.md", "hi\n"),
        ("Inbox/Robbery report.md", "hi\n"),
    ]);
    f.index();
    f
}

#[tokio::test]
async fn ranks_the_exact_name_first_and_reports_what_matched() {
    let f = vault();
    let out = f
        .ok("find_notes_by_name", json!({"query": "kelly criterion"}))
        .await;
    assert_eq!(
        out["results"][0],
        json!({
            "path": "Finance/Kelly criterion.md",
            "title": "Kelly criterion",
            "matchedOn": "name",
            "score": 1,
        })
    );
    assert_eq!(out["total"], 1);
    assert!(out.get("truncated").is_none());
    let both = f.ok("find_notes_by_name", json!({"query": "kelly"})).await;
    assert_eq!(
        paths(&both),
        ["Finance/Kelly notes.md", "Finance/Kelly criterion.md"]
    );
}

#[tokio::test]
async fn matches_the_frontmatter_title() {
    let f = vault();
    let out = f
        .ok("find_notes_by_name", json!({"query": "betting sizes"}))
        .await;
    let top = &out["results"][0];
    assert_eq!(top["path"], "Finance/Kelly notes.md");
    assert_eq!(top["matchedOn"], "title");
    assert_eq!(top["score"], 1);
}

#[tokio::test]
async fn matches_an_alias_and_names_it() {
    let f = vault();
    let out = f
        .ok("find_notes_by_name", json!({"query": "bobby tables"}))
        .await;
    assert_eq!(
        out["results"][0],
        json!({
            "path": "People/Bob Smith.md",
            "title": "Bob Smith",
            "matchedOn": "alias",
            "score": 1,
            "alias": "Bobby Tables",
        })
    );
}

#[tokio::test]
async fn an_exact_alias_outranks_a_prefix_match_on_another_name() {
    let f = vault();
    let out = f.ok("find_notes_by_name", json!({"query": "robert"})).await;
    assert_eq!(out["results"][0]["path"], "People/Bob Smith.md");
    assert_eq!(out["results"][0]["matchedOn"], "alias");
    assert_eq!(out["results"][1]["path"], "People/Robert Jones.md");
    assert_eq!(out["results"][1]["matchedOn"], "name");
}

#[tokio::test]
async fn matches_non_contiguous_characters_case_insensitively() {
    let f = vault();
    let out = f.ok("find_notes_by_name", json!({"query": "KLCRT"})).await;
    assert_eq!(out["results"][0]["path"], "Finance/Kelly criterion.md");
}

#[tokio::test]
async fn never_returns_stubs() {
    let f = vault();
    let out = f
        .ok("find_notes_by_name", json!({"query": "missing note"}))
        .await;
    assert_eq!(out["results"], json!([]));
    assert_eq!(out["total"], 0);
}

#[tokio::test]
async fn folder_narrows_the_candidates_and_a_trailing_slash_is_fine() {
    let f = vault();
    let out = f
        .ok(
            "find_notes_by_name",
            json!({"query": "rob", "folder": "People/"}),
        )
        .await;
    let p = paths(&out);
    assert!(!p.is_empty());
    assert!(p.iter().all(|p| p.starts_with("People/")));
}

#[tokio::test]
async fn the_vault_root_is_the_same_as_no_folder() {
    let f = vault();
    let all = f.ok("find_notes_by_name", json!({"query": "rob"})).await;
    let root = f
        .ok("find_notes_by_name", json!({"query": "rob", "folder": "/"}))
        .await;
    assert_eq!(root, all);
}

#[tokio::test]
async fn limit_truncates_and_total_keeps_the_full_count() {
    let f = vault();
    let out = f
        .ok("find_notes_by_name", json!({"query": "o", "limit": 2}))
        .await;
    assert_eq!(out["results"].as_array().unwrap().len(), 2);
    assert!(out["total"].as_u64().unwrap() > 2);
    assert_eq!(out["truncated"], true);
}

#[tokio::test]
async fn an_unknown_folder_is_an_error() {
    let f = vault();
    let e = f
        .err(
            "find_notes_by_name",
            json!({"query": "x", "folder": "Nope"}),
        )
        .await;
    assert_eq!(e, "Folder not found: \"Nope\"");
}

#[tokio::test]
async fn refuses_folders_that_leave_the_vault() {
    let f = vault();
    let e = f
        .err(
            "find_notes_by_name",
            json!({"query": "x", "folder": "../etc"}),
        )
        .await;
    assert!(e.contains("outside the vault"), "{e}");
    let e = f
        .err(
            "find_notes_by_name",
            json!({"query": "x", "folder": "C:/etc"}),
        )
        .await;
    assert!(e.contains("vault-relative"), "{e}");
}

#[tokio::test]
async fn validation_rejects_an_empty_query_and_an_out_of_range_limit() {
    let f = vault();
    assert!(!f.accepts("find_notes_by_name", json!({"query": ""})).await);
    assert!(
        !f.accepts("find_notes_by_name", json!({"query": "a", "limit": 101}))
            .await
    );
    assert!(
        f.accepts("find_notes_by_name", json!({"query": "a", "limit": 100}))
            .await
    );
}
