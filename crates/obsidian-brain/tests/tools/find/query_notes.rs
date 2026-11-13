use std::time::{Duration, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::support::{Fixture, fixture, paths};

fn set_mtime(f: &Fixture, rel: &str, iso_secs: u64) {
    let file = std::fs::File::options()
        .write(true)
        .open(f.vault().join(rel))
        .unwrap();
    file.set_modified(UNIX_EPOCH + Duration::from_secs(iso_secs))
        .unwrap();
}

fn vault() -> Fixture {
    let f = fixture(&[
        (
            "Books/Dune.md",
            "---\ntags: [Book, scifi]\nrating: 5\nstatus: read\nauthor: Herbert\n---\nSee [[Unwritten]].\n",
        ),
        (
            "Books/Emma.md",
            "---\ntags: book\nrating: 3\nstatus: reading\n---\nbody #Classic\n",
        ),
        (
            "Books/Notes/Index.md",
            "---\ntitle: Book index\n---\nrating:: 4\n",
        ),
        ("Areas/Work.md", "---\ntags: [area/work]\n---\nbody\n"),
        ("Root.md", "plain note\n"),
    ]);
    f.index();
    set_mtime(&f, "Root.md", 1_577_836_800); // 2020-01-01T00:00:00Z
    set_mtime(&f, "Books/Dune.md", 1_717_200_000); // 2024-06-01T00:00:00Z
    f
}

async fn query(f: &Fixture, args: Value) -> Value {
    f.ok("query_notes", args).await
}

#[tokio::test]
async fn filters_on_a_frontmatter_value_and_returns_the_note_record() {
    let f = vault();
    let out = query(
        &f,
        json!({"filter": {"==": [{"var": "frontmatter.status"}, "read"]}}),
    )
    .await;
    assert_eq!(out["total"], 1);
    let size = std::fs::metadata(f.vault().join("Books/Dune.md"))
        .unwrap()
        .len();
    assert_eq!(
        out["results"][0],
        json!({
            "path": "Books/Dune.md",
            "title": "Dune",
            "tags": ["Book", "scifi"],
            "mtime": "2024-06-01T00:00:00.000Z",
            "size": size,
            "frontmatter": {"tags": ["Book", "scifi"], "rating": 5, "status": "read", "author": "Herbert"},
        })
    );
    let keys: Vec<&String> = out["results"][0].as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        ["path", "title", "tags", "mtime", "size", "frontmatter"]
    );
}

#[tokio::test]
async fn a_constant_true_filter_matches_every_real_note_sorted_by_path() {
    let f = vault();
    let out = query(&f, json!({"filter": {"==": [1, 1]}})).await;
    assert_eq!(
        paths(&out),
        [
            "Areas/Work.md",
            "Books/Dune.md",
            "Books/Emma.md",
            "Books/Notes/Index.md",
            "Root.md"
        ]
    );
}

#[tokio::test]
async fn has_tag_is_case_insensitive_and_sees_inline_and_nested_tags() {
    let f = vault();
    let tag = |t: &str| json!({"filter": {"has_tag": [{"var": "tags"}, t]}});
    assert_eq!(
        paths(&query(&f, tag("#BOOK")).await),
        ["Books/Dune.md", "Books/Emma.md"]
    );
    assert_eq!(paths(&query(&f, tag("classic")).await), ["Books/Emma.md"]);
    assert_eq!(paths(&query(&f, tag("area")).await), ["Areas/Work.md"]);
    let bad = query(&f, json!({"filter": {"has_tag": [{"var": "title"}, "x"]}})).await;
    assert_eq!(bad["total"], 0);
}

#[tokio::test]
async fn exposes_path_folder_name_title_mtime_and_size() {
    let f = vault();
    let by = |filter: Value| json!({ "filter": filter });
    let folder = query(&f, by(json!({"==": [{"var": "folder"}, "Books/Notes"]}))).await;
    assert_eq!(paths(&folder), ["Books/Notes/Index.md"]);
    let root = query(&f, by(json!({"==": [{"var": "folder"}, ""]}))).await;
    assert_eq!(paths(&root), ["Root.md"]);
    let name = query(&f, by(json!({"==": [{"var": "name"}, "Index"]}))).await;
    assert_eq!(name["results"][0]["title"], "Book index");
    let under = query(&f, by(json!({"in": ["Books/", {"var": "path"}]}))).await;
    assert_eq!(under["total"], 3);
    let old = query(&f, by(json!({"<": [{"var": "mtime"}, "2021"]}))).await;
    assert_eq!(paths(&old), ["Root.md"]);
    let sized = query(&f, by(json!({">": [{"var": "size"}, 0]}))).await;
    assert_eq!(sized["total"], 5);
}

#[tokio::test]
async fn sees_dataview_inline_fields_but_not_the_inline_tags_field() {
    let f = vault();
    let out = query(
        &f,
        json!({"filter": {"==": [{"var": "frontmatter.rating"}, "4"]}}),
    )
    .await;
    assert_eq!(paths(&out), ["Books/Notes/Index.md"]);
    let emma = query(&f, json!({"filter": {"==": [{"var": "name"}, "Emma"]}})).await;
    assert!(
        emma["results"][0]["frontmatter"]
            .get("inline_tags")
            .is_none()
    );
    assert_eq!(emma["results"][0]["tags"], json!(["book", "Classic"]));
}

#[tokio::test]
async fn combines_conditions_with_and() {
    let f = vault();
    let out = query(
        &f,
        json!({"filter": {"and": [
            {"has_tag": [{"var": "tags"}, "book"]},
            {">=": [{"var": "frontmatter.rating"}, 4]}
        ]}}),
    )
    .await;
    assert_eq!(paths(&out), ["Books/Dune.md"]);
}

#[tokio::test]
async fn fields_projects_frontmatter_keys_and_skips_absent_ones() {
    let f = vault();
    let out = query(
        &f,
        json!({"filter": {"has_tag": [{"var": "tags"}, "book"]}, "fields": ["rating", "nope"]}),
    )
    .await;
    let fms: Vec<&Value> = out["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| &r["frontmatter"])
        .collect();
    assert_eq!(fms, [&json!({"rating": 5}), &json!({"rating": 3})]);
}

#[tokio::test]
async fn sorts_by_a_frontmatter_key_with_missing_values_last_in_both_orders() {
    let f = vault();
    let all = json!({"==": [1, 1]});
    let asc = query(
        &f,
        json!({"filter": all, "sort": {"field": "frontmatter.rating"}}),
    )
    .await;
    // Index has rating "4" (a string from an inline field); a string and a
    // number compare as text.
    assert_eq!(
        paths(&asc),
        [
            "Books/Emma.md",
            "Books/Notes/Index.md",
            "Books/Dune.md",
            "Areas/Work.md",
            "Root.md"
        ]
    );
    let desc = query(
        &f,
        json!({"filter": all, "sort": {"field": "frontmatter.rating", "order": "desc"}}),
    )
    .await;
    assert_eq!(
        paths(&desc),
        [
            "Books/Dune.md",
            "Books/Notes/Index.md",
            "Books/Emma.md",
            "Areas/Work.md",
            "Root.md"
        ]
    );
}

#[tokio::test]
async fn sorts_numbers_numerically() {
    let f = vault();
    let out = query(
        &f,
        json!({"filter": {"has_tag": [{"var": "tags"}, "book"]}, "sort": {"field": "frontmatter.rating", "order": "asc"}}),
    )
    .await;
    assert_eq!(paths(&out), ["Books/Emma.md", "Books/Dune.md"]);
}

#[tokio::test]
async fn sorts_by_title_and_by_mtime() {
    let f = vault();
    let all = json!({"==": [1, 1]});
    let by_title = query(&f, json!({"filter": all, "sort": {"field": "title"}})).await;
    assert_eq!(by_title["results"][0]["title"], "Book index");
    let newest = query(
        &f,
        json!({"filter": all, "sort": {"field": "mtime", "order": "desc"}, "limit": 5}),
    )
    .await;
    assert_eq!(paths(&newest).last().unwrap(), "Root.md");
}

#[tokio::test]
async fn limit_truncates_and_total_keeps_the_full_count() {
    let f = vault();
    let out = query(&f, json!({"filter": {"==": [1, 1]}, "limit": 2})).await;
    assert_eq!(out["results"].as_array().unwrap().len(), 2);
    assert_eq!(out["total"], 5);
    assert_eq!(out["truncated"], true);
}

#[tokio::test]
async fn skips_notes_the_index_has_but_the_disk_does_not() {
    let f = vault();
    f.upsert("Gone.md", "Gone", json!({}));
    let out = query(&f, json!({"filter": {"==": [1, 1]}})).await;
    assert!(!paths(&out).contains(&"Gone.md".to_string()));
}

#[tokio::test]
async fn rejects_a_filter_that_is_not_one_operator() {
    let f = vault();
    for filter in [json!({}), json!({"a": 1, "b": 2}), json!([1]), Value::Null] {
        let e = f.err("query_notes", json!({ "filter": filter })).await;
        assert!(e.contains("exactly one operator"), "{e}");
    }
}

#[tokio::test]
async fn reports_an_unknown_operator() {
    let f = vault();
    let e = f
        .err(
            "query_notes",
            json!({"filter": {"startsWith": [{"var": "path"}, "Books"]}}),
        )
        .await;
    assert_eq!(e, "Invalid filter: Unrecognized operation startsWith");
}

#[tokio::test]
async fn rejects_an_unknown_sort_field() {
    let f = vault();
    for field in ["size", "frontmatter."] {
        let e = f
            .err(
                "query_notes",
                json!({"filter": {"==": [1, 1]}, "sort": {"field": field}}),
            )
            .await;
        assert!(e.contains("sort.field"), "{e}");
    }
}

#[tokio::test]
async fn validation_requires_filter_and_bounds_limit() {
    let f = vault();
    assert!(!f.accepts("query_notes", json!({})).await);
    assert!(
        !f.accepts(
            "query_notes",
            json!({"filter": {"==": [1, 1]}, "limit": 501})
        )
        .await
    );
    assert!(
        !f.accepts(
            "query_notes",
            json!({"filter": {"==": [1, 1]}, "sort": {"field": "path", "order": "up"}})
        )
        .await
    );
    assert!(
        f.accepts(
            "query_notes",
            json!({"filter": {"==": [1, 1]}, "limit": 500, "fields": ["a"]})
        )
        .await
    );
}
