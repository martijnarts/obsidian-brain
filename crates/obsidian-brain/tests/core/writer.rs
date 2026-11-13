//! Tests of the vault writer.

use obsidian_brain::store::Db;
use obsidian_brain::store::edges::get_edges_by_source;
use obsidian_brain::store::nodes::get_node;
use obsidian_brain::vault::writer::{CreateNodeOptions, VaultWriter};
use serde_json::json;

use crate::support::{copy_dir, fixture_vault, memory_db};

struct Fixture {
    dir: tempfile::TempDir,
    db: Db,
    writer: VaultWriter,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        copy_dir(&fixture_vault(), dir.path());
        let writer = VaultWriter::new(dir.path());
        Self {
            dir,
            db: memory_db(),
            writer,
        }
    }

    fn create(
        &self,
        title: &str,
        directory: Option<&str>,
        fm: serde_json::Value,
        content: &str,
    ) -> anyhow::Result<String> {
        let opts = CreateNodeOptions {
            title: title.into(),
            directory: directory.map(Into::into),
            frontmatter: fm.as_object().unwrap().clone(),
            content: content.into(),
        };
        self.db.with(|c| self.writer.create_node(c, &opts))
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.dir.path().join(rel)).unwrap()
    }
}

#[test]
fn create_node_writes_frontmatter_and_indexes() {
    let f = Fixture::new();
    let rel = f
        .create(
            "New Concept",
            Some("Concepts"),
            json!({"type": "concept", "tags": ["test"]}),
            "This is a new concept about testing.",
        )
        .unwrap();
    assert_eq!(rel, "Concepts/New Concept.md");
    let raw = f.read("Concepts/New Concept.md");
    assert!(raw.contains("title: New Concept"));
    assert!(raw.contains("type: concept"));
    assert!(raw.contains("This is a new concept about testing."));
    let node =
        f.db.with(|c| get_node(c, "Concepts/New Concept.md"))
            .unwrap()
            .unwrap();
    assert_eq!(node.title, "New Concept");
}

#[test]
fn create_node_makes_directories_and_defaults_to_the_root() {
    let f = Fixture::new();
    f.create(
        "Fresh Note",
        Some("NewDir"),
        json!({}),
        "In a new directory.",
    )
    .unwrap();
    assert!(f.dir.path().join("NewDir/Fresh Note.md").exists());
    f.create("Root Note", None, json!({}), "At the root.")
        .unwrap();
    assert!(f.dir.path().join("Root Note.md").exists());
}

#[test]
fn create_node_refuses_an_existing_file() {
    let f = Fixture::new();
    let err = f
        .create("Alice Smith", Some("People"), json!({}), "Duplicate.")
        .unwrap_err();
    assert!(err.to_string().contains("already exists"));
}

#[test]
fn annotate_node_appends_and_reindexes() {
    let f = Fixture::new();
    f.db.with(|c| {
        f.writer.annotate_node(
            c,
            "People/Alice Smith.md",
            "\n## Agent Notes\nAlice is a key connector.",
        )
    })
    .unwrap();
    let raw = f.read("People/Alice Smith.md");
    assert!(raw.contains("## Agent Notes"));
    assert!(raw.contains("Alice is a key connector."));

    f.create("Temp Note", None, json!({}), "Original content.")
        .unwrap();
    let before = f.db.with(|c| get_node(c, "Temp Note.md")).unwrap().unwrap();
    assert!(before.content.contains("Original content."));
    f.db.with(|c| {
        f.writer
            .annotate_node(c, "Temp Note.md", "\n\nAppended content.")
    })
    .unwrap();
    let after = f.db.with(|c| get_node(c, "Temp Note.md")).unwrap().unwrap();
    assert!(after.content.contains("Appended content."));

    let err =
        f.db.with(|c| f.writer.annotate_node(c, "nonexistent.md", "stuff"))
            .unwrap_err();
    assert!(err.to_string().contains("not found"));
}

#[test]
fn add_link_appends_the_link_and_stores_the_edge() {
    let f = Fixture::new();
    f.create("Source", None, json!({}), "Some content.")
        .unwrap();
    f.db.with(|c| {
        f.writer
            .add_link(c, "Source.md", "People/Alice Smith", "Related to Alice.")
    })
    .unwrap();
    let raw = f.read("Source.md");
    assert!(raw.contains("[[People/Alice Smith]]"));
    assert!(raw.contains("Related to Alice."));
    let edges = f.db.with(|c| get_edges_by_source(c, "Source.md")).unwrap();
    assert!(edges.iter().any(|e| e.target_id == "People/Alice Smith.md"));
}

#[test]
fn title_injection_and_its_opt_out() {
    let f = Fixture::new();
    f.create("AutoTitleTest", None, json!({"tags": ["a"]}), "body")
        .unwrap();
    assert!(f.read("AutoTitleTest.md").contains("title: AutoTitleTest"));

    f.create(
        "NoTitle",
        None,
        json!({"title": null, "tags": ["a"]}),
        "body",
    )
    .unwrap();
    let raw = f.read("NoTitle.md");
    assert!(!raw.contains("title:"));
    assert!(raw.contains("tags:"));

    f.create(
        "Filename",
        None,
        json!({"title": "Human Readable Title"}),
        "body",
    )
    .unwrap();
    assert!(
        f.read("Filename.md")
            .contains("title: Human Readable Title")
    );
}

#[test]
fn create_node_refuses_a_path_outside_the_vault() {
    let f = Fixture::new();
    assert!(
        f.create("escape", Some("../outside"), json!({}), "x")
            .is_err()
    );
}
