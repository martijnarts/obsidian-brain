//! Ported from `test/tools/update-properties.test.ts`.

use serde_json::json;

use super::harness::{Harness, assert_contains, outside, symlink};

const BODY: &str = "# Heading\n\nBody text with [[link]].\n";

fn note(h: &Harness, rel: &str, content: &str) {
    note_titled(h, rel, content, rel.trim_end_matches(".md"));
}

fn note_titled(h: &Harness, rel: &str, content: &str, title: &str) {
    h.write(rel, content);
    h.node(rel, title, "", json!({}));
}

#[tokio::test]
async fn sets_and_removes_several_keys_in_one_write() {
    let h = Harness::new();
    note(
        &h,
        "n.md",
        &format!("---\ntitle: Note\nstatus: draft\nold: 1\ntags:\n  - a\n---\n{BODY}"),
    );
    let out = h
        .ok(
            "update_properties",
            json!({ "name": "n.md", "set": { "status": "done", "rating": 4, "tags": ["a", "b"] }, "remove": ["old"] }),
        )
        .await;
    assert_eq!(
        out,
        json!({
            "path": "n.md",
            "set": ["status", "rating", "tags"],
            "removed": ["old"],
            "frontmatter": { "title": "Note", "status": "done", "tags": ["a", "b"], "rating": 4 },
        })
    );
    assert_eq!(
        h.read("n.md"),
        format!("---\ntitle: Note\nstatus: done\ntags:\n  - a\n  - b\nrating: 4\n---\n{BODY}")
    );
    assert_eq!(h.reindexes(), 1);
    h.settle().await;
}

#[tokio::test]
async fn creates_the_frontmatter_block_when_the_note_has_none() {
    let h = Harness::new();
    note(&h, "plain.md", BODY);
    h.ok(
        "update_properties",
        json!({ "name": "plain", "set": { "status": "new" } }),
    )
    .await;
    assert_eq!(h.read("plain.md"), format!("---\nstatus: new\n---\n{BODY}"));
    h.settle().await;
}

#[tokio::test]
async fn removing_the_last_key_drops_the_block() {
    let h = Harness::new();
    note(&h, "one.md", &format!("---\nk: v\n---\n{BODY}"));
    h.ok(
        "update_properties",
        json!({ "name": "one.md", "remove": ["k"] }),
    )
    .await;
    assert_eq!(h.read("one.md"), BODY);
    h.settle().await;
}

#[tokio::test]
async fn a_null_value_removes_the_key_and_set_wins_over_remove() {
    let h = Harness::new();
    note(&h, "n.md", &format!("---\na: 1\nb: 2\n---\n{BODY}"));
    let out = h
        .ok(
            "update_properties",
            json!({ "name": "n.md", "set": { "a": null, "b": 3 }, "remove": ["b"] }),
        )
        .await;
    assert_eq!(out["removed"], json!(["a"]));
    assert_eq!(out["set"], json!(["b"]));
    assert_eq!(out["frontmatter"], json!({ "b": 3 }));
    h.settle().await;
}

#[tokio::test]
async fn reports_absent_keys_and_leaves_the_file_untouched_when_nothing_changes() {
    let h = Harness::new();
    let original = format!("---\nz:   'spaced'\n---\n{BODY}");
    note(&h, "n.md", &original);
    let out = h
        .ok(
            "update_properties",
            json!({ "name": "n.md", "remove": ["missing"] }),
        )
        .await;
    assert_eq!(out["notPresent"], json!(["missing"]));
    assert_eq!(out["removed"], json!([]));
    assert_eq!(h.read("n.md"), original);
    assert_eq!(h.reindexes(), 0);
}

#[tokio::test]
async fn dry_run_returns_the_new_frontmatter_and_does_not_write() {
    let h = Harness::new();
    let original = format!("---\na: 1\n---\n{BODY}");
    note(&h, "n.md", &original);
    let out = h
        .ok(
            "update_properties",
            json!({ "name": "n.md", "set": { "b": true }, "dryRun": true }),
        )
        .await;
    assert_eq!(out["dryRun"], true);
    assert_eq!(out["frontmatter"], json!({ "a": 1, "b": true }));
    assert_eq!(
        serde_json::to_string(&out).unwrap(),
        r#"{"dryRun":true,"path":"n.md","set":["b"],"removed":[],"frontmatter":{"a":1,"b":true}}"#
    );
    assert_eq!(h.read("n.md"), original);
    assert_eq!(h.reindexes(), 0);
}

#[tokio::test]
async fn resolves_a_note_by_title() {
    let h = Harness::new();
    note_titled(&h, "Sub/Long name.md", BODY, "Long name");
    h.ok(
        "update_properties",
        json!({ "name": "Long name", "set": { "x": 1 } }),
    )
    .await;
    assert!(h.read("Sub/Long name.md").contains("x: 1"));
    h.settle().await;
}

#[tokio::test]
async fn rejects_calls_with_nothing_to_do_and_invalid_keys() {
    let h = Harness::new();
    note(&h, "n.md", BODY);
    for args in [
        json!({ "name": "n.md" }),
        json!({ "name": "n.md", "set": {}, "remove": [] }),
    ] {
        assert_eq!(
            h.err("update_properties", args).await,
            "Nothing to do: pass at least one key in `set` or `remove`."
        );
    }
    assert_eq!(
        h.err(
            "update_properties",
            json!({ "name": "n.md", "set": { " ": 1 } })
        )
        .await,
        "Invalid frontmatter key(s): \" \""
    );
    assert_eq!(
        h.err(
            "update_properties",
            json!({ "name": "n.md", "remove": ["a\nb"] })
        )
        .await,
        "Invalid frontmatter key(s): \"a\\nb\""
    );
}

#[tokio::test]
async fn errors_on_a_missing_note_an_ambiguous_name_and_a_stub() {
    let h = Harness::new();
    note_titled(&h, "Alpha one.md", BODY, "Alpha one");
    note_titled(&h, "Alpha two.md", BODY, "Alpha two");
    h.node("_stub/Ghost.md", "Ghost", "", json!({ "_stub": true }));
    let cases = [
        ("nothing here", "No note found"),
        ("alpha", "Multiple notes match"),
        ("_stub/Ghost.md", "unresolved link target"),
    ];
    for (name, want) in cases {
        assert_contains(
            &h.err(
                "update_properties",
                json!({ "name": name, "set": { "a": 1 } }),
            )
            .await,
            &[want],
        );
    }
}

#[tokio::test]
async fn refuses_a_note_whose_file_is_a_symlink_out_of_the_vault() {
    let h = Harness::new();
    let out = outside();
    let target = out.path().join("secret.md");
    std::fs::write(&target, BODY).unwrap();
    symlink(&target, &h.abs("link.md"));
    h.node("link.md", "link", "", json!({}));
    assert_contains(
        &h.err(
            "update_properties",
            json!({ "name": "link.md", "set": { "a": 1 } }),
        )
        .await,
        &["outside the vault"],
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), BODY);
}

#[tokio::test]
async fn refuses_an_indexed_id_that_climbs_out_of_the_vault() {
    let h = Harness::new();
    let out = outside();
    std::fs::write(out.path().join("x.md"), BODY).unwrap();
    let rel = format!(
        "../../{}/x.md",
        out.path().file_name().unwrap().to_string_lossy()
    );
    h.node(&rel, "x", "", json!({}));
    assert_contains(
        &h.err(
            "update_properties",
            json!({ "name": rel, "set": { "a": 1 } }),
        )
        .await,
        &["outside the vault"],
    );
}
