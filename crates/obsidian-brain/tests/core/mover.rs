//! Ported from `test/vault/mover.test.ts`.

use std::path::Path;

use obsidian_brain::store::edges::{get_edges_by_source, insert_edge};
use obsidian_brain::store::embeddings::{search_vector, upsert_embedding};
use obsidian_brain::store::nodes::{get_node, upsert_node};
use obsidian_brain::store::sync::{get_sync_mtime, set_sync_mtime};
use obsidian_brain::types::{ParsedEdge, ParsedNode};
use obsidian_brain::vault::mover::{delete_note, move_note};
use serde_json::json;

use crate::support::{memory_db, write};

fn read(vault: &Path, rel: &str) -> String {
    std::fs::read_to_string(vault.join(rel)).unwrap()
}

fn source_vault() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "Notes/source.md", "# source\n");
    dir
}

#[test]
fn renames_in_place() {
    let dir = source_vault();
    let r = move_note(dir.path(), "Notes/source.md", "Notes/dest.md").unwrap();
    assert_eq!(
        (r.old_path.as_str(), r.new_path.as_str()),
        ("Notes/source.md", "Notes/dest.md")
    );
    assert_eq!(read(dir.path(), "Notes/dest.md"), "# source\n");
    assert!(!dir.path().join("Notes/source.md").exists());
}

#[test]
fn moves_into_new_directories() {
    let dir = source_vault();
    let r = move_note(
        dir.path(),
        "Notes/source.md",
        "Deep/Nested/newdir/source.md",
    )
    .unwrap();
    assert_eq!(r.new_path, "Deep/Nested/newdir/source.md");
    assert_eq!(
        read(dir.path(), "Deep/Nested/newdir/source.md"),
        "# source\n"
    );
}

#[test]
fn appends_md_and_reads_a_trailing_slash_as_a_directory() {
    let dir = source_vault();
    let r = move_note(dir.path(), "Notes/source.md", "Notes/renamed").unwrap();
    assert_eq!(r.new_path, "Notes/renamed.md");
    let r = move_note(dir.path(), "Notes/renamed.md", "Archive/").unwrap();
    assert_eq!(r.new_path, "Archive/renamed.md");
    assert_eq!(read(dir.path(), "Archive/renamed.md"), "# source\n");
}

#[test]
fn a_missing_source_is_an_error() {
    let dir = source_vault();
    let err = move_note(dir.path(), "missing.md", "dest.md").unwrap_err();
    assert!(err.to_string().contains("not found"));
}

#[test]
fn a_destination_outside_the_vault_is_refused() {
    let dir = source_vault();
    assert!(move_note(dir.path(), "Notes/source.md", "../escape.md").is_err());
    assert!(dir.path().join("Notes/source.md").exists());
}

#[test]
fn syncs_a_title_that_tracked_the_old_basename() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "foo.md", "---\ntitle: foo\n---\n\nBody.\n");
    move_note(dir.path(), "foo.md", "bar.md").unwrap();
    let moved = read(dir.path(), "bar.md");
    assert!(
        regex::Regex::new(r"title:\s*bar\b")
            .unwrap()
            .is_match(&moved),
        "{moved}"
    );
    assert!(moved.contains("Body."));
}

#[test]
fn title_sync_edits_only_the_title_line() {
    let dir = tempfile::tempdir().unwrap();
    let raw = "---\n# keep me\ntitle: foo\ntags: [a, b]\n---\n\nBody.\n";
    write(dir.path(), "foo.md", raw);
    move_note(dir.path(), "foo.md", "bar.md").unwrap();
    assert_eq!(
        read(dir.path(), "bar.md"),
        raw.replace("title: foo", "title: bar")
    );
}

#[test]
fn leaves_a_custom_title_and_adds_none() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "foo.md",
        "---\ntitle: Custom Title\n---\n\nBody.\n",
    );
    move_note(dir.path(), "foo.md", "bar.md").unwrap();
    assert!(read(dir.path(), "bar.md").contains("title: Custom Title"));

    write(dir.path(), "baz.md", "---\ntags: [a]\n---\n\nBody.\n");
    move_note(dir.path(), "baz.md", "qux.md").unwrap();
    let moved = read(dir.path(), "qux.md");
    assert!(!regex::Regex::new(r"(?m)^title:").unwrap().is_match(&moved));
    assert!(moved.contains("tags:"));
}

fn node(id: &str, fm: serde_json::Value) -> ParsedNode {
    ParsedNode {
        id: id.into(),
        title: id.into(),
        content: String::new(),
        frontmatter: fm.as_object().unwrap().clone(),
    }
}

fn edge(source: &str, target: &str, context: &str) -> ParsedEdge {
    ParsedEdge {
        source_id: source.into(),
        target_id: target.into(),
        context: context.into(),
        target_subpath: None,
    }
}

fn unit() -> Vec<f32> {
    let mut v = vec![0.0f32; 384];
    v[0] = 1.0;
    v
}

#[test]
fn delete_note_removes_the_file_and_its_index_rows() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "victim.md", "# victim\n");
    let db = memory_db();
    db.with(|c| {
        upsert_node(c, &node("victim.md", json!({})))?;
        upsert_node(c, &node("other.md", json!({})))?;
        insert_edge(c, &edge("victim.md", "other.md", "link"))?;
        insert_edge(c, &edge("victim.md", "other.md", "link 2"))?;
        upsert_embedding(c, "victim.md", &unit())?;
        set_sync_mtime(c, "victim.md", 1234.0, None)
    })
    .unwrap();

    let r = db
        .with(|c| delete_note(dir.path(), "victim.md", c))
        .unwrap();
    assert_eq!(r.path, "victim.md");
    assert!(r.deleted_from_index.node);
    assert_eq!(r.deleted_from_index.edges, 2);
    assert!(r.deleted_from_index.embedding);
    assert_eq!(r.deleted_from_index.stubs_pruned, 0);
    assert!(!dir.path().join("victim.md").exists());
    assert!(
        db.with(|c| get_edges_by_source(c, "victim.md"))
            .unwrap()
            .is_empty()
    );
    assert!(
        db.with(|c| get_sync_mtime(c, "victim.md"))
            .unwrap()
            .is_none()
    );
    let hits = db.with(|c| search_vector(c, &unit(), 10)).unwrap();
    assert!(!hits.iter().any(|h| h.node_id == "victim.md"));
    assert_eq!(
        serde_json::to_value(&r).unwrap(),
        json!({"path": "victim.md", "deletedFromIndex": {"node": true, "edges": 2, "embedding": true, "stubsPruned": 0}})
    );
}

#[test]
fn delete_note_reconciles_the_index_when_the_file_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let db = memory_db();
    db.with(|c| upsert_node(c, &node("victim.md", json!({}))))
        .unwrap();
    let r = db
        .with(|c| delete_note(dir.path(), "victim.md", c))
        .unwrap();
    assert!(r.deleted_from_index.node);
}

#[test]
fn delete_note_prunes_a_stub_only_when_it_was_the_last_referencer() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "noter.md", "# noter\n[[_stub/missing]]\n");
    let db = memory_db();
    db.with(|c| {
        upsert_node(c, &node("noter.md", json!({})))?;
        upsert_node(c, &node("_stub/missing", json!({"_stub": true})))?;
        insert_edge(c, &edge("noter.md", "_stub/missing", "link"))
    })
    .unwrap();
    let r = db.with(|c| delete_note(dir.path(), "noter.md", c)).unwrap();
    assert_eq!(r.deleted_from_index.stubs_pruned, 1);
    assert!(db.with(|c| get_node(c, "_stub/missing")).unwrap().is_none());

    write(dir.path(), "a.md", "# a\n[[_stub/shared]]\n");
    write(dir.path(), "b.md", "# b\n[[_stub/shared]]\n");
    db.with(|c| {
        upsert_node(c, &node("a.md", json!({})))?;
        upsert_node(c, &node("b.md", json!({})))?;
        upsert_node(c, &node("_stub/shared", json!({"_stub": true})))?;
        insert_edge(c, &edge("a.md", "_stub/shared", "link"))?;
        insert_edge(c, &edge("b.md", "_stub/shared", "link"))
    })
    .unwrap();
    let r = db.with(|c| delete_note(dir.path(), "a.md", c)).unwrap();
    assert_eq!(r.deleted_from_index.stubs_pruned, 0);
    assert!(db.with(|c| get_node(c, "_stub/shared")).unwrap().is_some());
}
