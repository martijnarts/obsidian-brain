use std::path::Path;
use std::sync::Arc;

use obsidian_brain::context::VaultContext;
use regex::Regex;
use serde_json::{Value, json};

use super::{context, err, ok};

fn board() -> Value {
    json!({
        "nodes": [
            { "id": "aaaaaaaaaaaaaaaa", "type": "text", "text": "Hello", "x": 0, "y": 0, "width": 250, "height": 60, "styleAttributes": { "textAlign": "center" } },
            { "id": "bbbbbbbbbbbbbbbb", "type": "file", "file": "Notes/Idea.md", "x": 300, "y": -40, "width": 400, "height": 400 },
            { "id": "cccccccccccccccc", "type": "group", "label": "Box", "x": -100, "y": 500, "width": 200, "height": 100 },
        ],
        "edges": [
            { "id": "eeeeeeeeeeeeeeee", "fromNode": "aaaaaaaaaaaaaaaa", "fromSide": "right", "toNode": "bbbbbbbbbbbbbbbb", "toSide": "left", "custom": 1 },
        ],
    })
}

struct Fixture {
    dir: tempfile::TempDir,
    ctx: Arc<VaultContext>,
}

impl Fixture {
    fn vault(&self) -> &Path {
        self.dir.path()
    }

    fn write(&self, rel: &str, content: &str) {
        super::support::write(self.vault(), rel, content);
    }

    fn raw(&self, rel: &str) -> String {
        std::fs::read_to_string(self.vault().join(rel)).unwrap()
    }

    fn doc(&self, rel: &str) -> Value {
        serde_json::from_str(&self.raw(rel)).unwrap()
    }

    async fn read(&self, args: Value) -> Value {
        ok(&self.ctx, "read_canvas", args).await
    }

    async fn read_err(&self, args: Value) -> String {
        err(&self.ctx, "read_canvas", args).await
    }

    async fn edit(&self, args: Value) -> Value {
        ok(&self.ctx, "edit_canvas", args).await
    }

    async fn edit_err(&self, args: Value) -> String {
        err(&self.ctx, "edit_canvas", args).await
    }
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let ctx = context(dir.path());
    let f = Fixture { dir, ctx };
    f.write("Notes/Idea.md", "# Idea\n");
    f.write(
        "Board.canvas",
        &serde_json::to_string_pretty(&board()).unwrap(),
    );
    f
}

fn assert_matches(text: &str, pattern: &str) {
    assert!(
        Regex::new(pattern).unwrap().is_match(text),
        "{text:?} !~ /{pattern}/"
    );
}

/// `toMatchObject`: every key of `want` is in `got` with the same value.
fn assert_subset(got: &Value, want: Value) {
    for (k, v) in want.as_object().unwrap() {
        assert_eq!(&got[k], v, "key {k} of {got}");
    }
}

// read_canvas

#[tokio::test]
async fn read_returns_nodes_and_edges_with_unknown_fields() {
    let f = setup();
    let out = f.read(json!({ "path": "Board.canvas" })).await;
    assert_eq!(out["path"], "Board.canvas");
    assert_eq!(out["nodes"], board()["nodes"]);
    assert_eq!(out["edges"], board()["edges"]);
}

#[tokio::test]
async fn read_appends_the_extension() {
    let f = setup();
    let out = f.read(json!({ "path": "Board" })).await;
    assert_eq!(out["nodes"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn read_treats_empty_files_and_missing_keys_as_empty() {
    let f = setup();
    f.write("Empty.canvas", "");
    assert_eq!(
        f.read(json!({ "path": "Empty.canvas" })).await,
        json!({ "path": "Empty.canvas", "nodes": [], "edges": [] })
    );
    f.write("Half.canvas", r#"{"nodes":[]}"#);
    assert_eq!(f.read(json!({ "path": "Half" })).await["edges"], json!([]));
}

#[tokio::test]
async fn read_errors_on_missing_and_malformed_canvases() {
    let f = setup();
    assert_matches(
        &f.read_err(json!({ "path": "Nope.canvas" })).await,
        r"Canvas not found: Nope\.canvas",
    );
    f.write("Bad.canvas", "{nodes: [");
    assert_matches(
        &f.read_err(json!({ "path": "Bad.canvas" })).await,
        r"Bad\.canvas is not valid JSON",
    );
    f.write("Arr.canvas", "[]");
    assert_matches(
        &f.read_err(json!({ "path": "Arr.canvas" })).await,
        "not a JSON object",
    );
    f.write("Odd.canvas", r#"{"nodes":{},"edges":[]}"#);
    assert_matches(
        &f.read_err(json!({ "path": "Odd.canvas" })).await,
        r#"non-array "nodes""#,
    );
}

#[tokio::test]
async fn read_refuses_paths_outside_the_vault() {
    let f = setup();
    assert_matches(
        &f.read_err(json!({ "path": "../outside.canvas" })).await,
        "outside the vault",
    );
    assert_matches(
        &f.read_err(json!({ "path": "/etc/x.canvas" })).await,
        "vault-relative",
    );
    assert_matches(
        &f.read_err(json!({ "path": "" })).await,
        "at least 1 character",
    );

    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("Secret.canvas"), "{}").unwrap();
    std::os::unix::fs::symlink(outside.path(), f.vault().join("link")).unwrap();
    assert_matches(
        &f.read_err(json!({ "path": "link/Secret.canvas" })).await,
        "through a symlink",
    );
}

// edit_canvas add_node

#[tokio::test]
async fn add_places_a_text_node_right_of_the_right_most_node() {
    let f = setup();
    let out = f
        .edit(json!({ "path": "Board.canvas", "operation": "add_node", "type": "text", "text": "New" }))
        .await;
    assert_subset(
        &out["node"],
        json!({ "type": "text", "text": "New", "x": 740, "y": -40, "width": 400, "height": 200 }),
    );
    assert_matches(out["node"]["id"].as_str().unwrap(), "^[0-9a-f]{16}$");
    assert_eq!(out["created"], false);
    let doc = f.doc("Board.canvas");
    assert_eq!(doc["nodes"].as_array().unwrap().len(), 4);
    assert_eq!(doc["nodes"][3], out["node"]);
    assert_eq!(doc["nodes"][0], board()["nodes"][0]);
    assert_eq!(doc["edges"], board()["edges"]);
}

#[tokio::test]
async fn add_writes_the_obsidian_layout() {
    let f = setup();
    f.edit(
        json!({ "path": "Board.canvas", "operation": "add_node", "type": "text", "text": "New" }),
    )
    .await;
    let raw = f.raw("Board.canvas");
    let doc = f.doc("Board.canvas");
    let lines: Vec<&str> = raw.split('\n').collect();
    assert_eq!(lines[0], "{");
    assert_eq!(lines[1], "\t\"nodes\":[");
    assert_eq!(
        lines[2],
        format!("\t\t{},", serde_json::to_string(&doc["nodes"][0]).unwrap())
    );
    assert!(raw.contains(&format!(
        "\t\"edges\":[\n\t\t{}\n\t]\n}}",
        serde_json::to_string(&doc["edges"][0]).unwrap()
    )));
}

#[tokio::test]
async fn add_keeps_extra_top_level_keys() {
    let f = setup();
    f.write(
        "Meta.canvas",
        r#"{"nodes":[],"edges":[],"metadata":{"v":1}}"#,
    );
    f.edit(json!({ "path": "Meta.canvas", "operation": "add_node", "type": "group" }))
        .await;
    assert_eq!(f.doc("Meta.canvas")["metadata"], json!({ "v": 1 }));
    assert!(f.raw("Meta.canvas").contains("\t\"metadata\":{\"v\":1}"));
}

#[tokio::test]
async fn add_creates_a_missing_canvas_and_its_folders() {
    let f = setup();
    let out = f
        .edit(json!({ "path": "Boards/New", "operation": "add_node", "type": "link", "url": "https://example.com" }))
        .await;
    assert_eq!(out["path"], "Boards/New.canvas");
    assert_eq!(out["created"], true);
    assert_subset(
        &out["node"],
        json!({ "type": "link", "url": "https://example.com", "x": 0, "y": 0 }),
    );
    assert_eq!(f.doc("Boards/New.canvas")["nodes"], json!([out["node"]]));
    assert!(f.raw("Boards/New.canvas").contains("\t\"edges\":[]"));
    let tmp = std::fs::read_dir(f.vault().join("Boards"))
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        })
        .count();
    assert_eq!(tmp, 0);
}

#[tokio::test]
async fn add_honours_position_size_color_and_label() {
    let f = setup();
    let out = f
        .edit(json!({
            "path": "Board.canvas", "operation": "add_node", "type": "group",
            "label": "Cluster", "color": "4", "x": 10, "y": 20, "width": 800, "height": 600,
        }))
        .await;
    assert_subset(
        &out["node"],
        json!({ "type": "group", "label": "Cluster", "color": "4", "x": 10, "y": 20, "width": 800, "height": 600 }),
    );
    let keys: Vec<&String> = out["node"].as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        ["id", "type", "x", "y", "width", "height", "label", "color"]
    );
}

#[tokio::test]
async fn add_file_nodes_warn_only_when_the_target_is_missing() {
    let f = setup();
    let out = f
        .edit(json!({ "path": "Board.canvas", "operation": "add_node", "type": "file", "file": "Notes/Idea.md", "subpath": "#Idea" }))
        .await;
    assert_subset(
        &out["node"],
        json!({ "file": "Notes/Idea.md", "subpath": "#Idea" }),
    );
    assert!(out.get("warnings").is_none());

    let out = f
        .edit(json!({ "path": "Board.canvas", "operation": "add_node", "type": "file", "file": "Missing.md" }))
        .await;
    assert_eq!(
        out["warnings"],
        json!(["File not found in the vault: Missing.md"])
    );
    assert_eq!(f.doc("Board.canvas")["nodes"].as_array().unwrap().len(), 5);
}

#[tokio::test]
async fn add_refuses_a_file_node_outside_the_vault() {
    let f = setup();
    let e = f
        .edit_err(json!({ "path": "Board.canvas", "operation": "add_node", "type": "file", "file": "../x.md" }))
        .await;
    assert_matches(&e, "outside the vault");
    assert_eq!(f.doc("Board.canvas")["nodes"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn add_ignores_null_optional_fields() {
    let f = setup();
    let out = f
        .edit(json!({ "path": "Board.canvas", "operation": "add_node", "type": "text", "text": "x", "color": null }))
        .await;
    assert!(out["node"].get("color").is_none());
}

#[tokio::test]
async fn add_requires_a_type_and_its_content_field() {
    let f = setup();
    let cases = [
        (json!({ "text": "x" }), "needs `type`"),
        (json!({ "type": "text" }), "text node needs `text`"),
        (json!({ "type": "file" }), "file node needs `file`"),
        (json!({ "type": "link" }), "link node needs `url`"),
    ];
    for (extra, pattern) in cases {
        let mut args = json!({ "path": "Board.canvas", "operation": "add_node" });
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_matches(&f.edit_err(args).await, pattern);
    }
}

#[tokio::test]
async fn add_refuses_fields_of_another_type_or_operation() {
    let f = setup();
    assert_matches(
        &f.edit_err(json!({ "path": "Board.canvas", "operation": "add_node", "type": "text", "text": "x", "url": "https://a" }))
            .await,
        "`url` does not apply to a text node",
    );
    assert_matches(
        &f.edit_err(json!({ "path": "Board.canvas", "operation": "add_node", "type": "text", "text": "x", "edgeId": "e" }))
            .await,
        "`edgeId` does not apply to add_node",
    );
}

#[tokio::test]
async fn add_generates_unique_ids() {
    let f = setup();
    let mut ids = std::collections::HashSet::new();
    for i in 0..5 {
        let out = f
            .edit(json!({ "path": "Board.canvas", "operation": "add_node", "type": "text", "text": i.to_string() }))
            .await;
        ids.insert(out["node"]["id"].as_str().unwrap().to_string());
    }
    assert_eq!(ids.len(), 5);
}

#[tokio::test]
async fn add_places_next_to_nodes_without_geometry() {
    let f = setup();
    f.write(
        "Loose.canvas",
        r#"{"nodes":[{"id":"x","type":"text","text":"a"}],"edges":[]}"#,
    );
    let out = f
        .edit(json!({ "path": "Loose", "operation": "add_node", "type": "text", "text": "b" }))
        .await;
    assert_subset(&out["node"], json!({ "x": 40, "y": 0 }));
}

#[tokio::test]
async fn add_refuses_a_non_positive_size() {
    let f = setup();
    let e = f
        .edit_err(
            json!({ "path": "Board.canvas", "operation": "add_node", "type": "group", "width": 0 }),
        )
        .await;
    assert_matches(&e, "positive number");
}

// edit_canvas update_node

#[tokio::test]
async fn update_changes_only_the_named_fields() {
    let f = setup();
    let out = f
        .edit(json!({ "path": "Board.canvas", "operation": "update_node", "nodeId": "aaaaaaaaaaaaaaaa", "text": "Bye", "x": 5, "color": "#ff0000" }))
        .await;
    let mut want = board()["nodes"][0].clone();
    want["text"] = json!("Bye");
    want["x"] = json!(5);
    want["color"] = json!("#ff0000");
    assert_eq!(out["node"], want);
    assert_eq!(f.doc("Board.canvas")["nodes"][0], out["node"]);
}

#[tokio::test]
async fn update_removes_fields_given_as_null() {
    let f = setup();
    f.edit(json!({ "path": "Board.canvas", "operation": "update_node", "nodeId": "cccccccccccccccc", "label": null }))
        .await;
    assert!(f.doc("Board.canvas")["nodes"][2].get("label").is_none());
}

#[tokio::test]
async fn update_refuses_to_remove_a_required_field() {
    let f = setup();
    let e = f
        .edit_err(json!({ "path": "Board.canvas", "operation": "update_node", "nodeId": "aaaaaaaaaaaaaaaa", "subpath": null }))
        .await;
    assert_matches(&e, "`subpath` does not apply to a text node");
    let e = f
        .edit_err(json!({ "path": "Board.canvas", "operation": "update_node", "nodeId": "bbbbbbbbbbbbbbbb", "file": "x.md", "label": null }))
        .await;
    assert_matches(&e, "`label` does not apply to a file node");
}

#[tokio::test]
async fn update_warns_on_a_missing_file_target() {
    let f = setup();
    let out = f
        .edit(json!({ "path": "Board.canvas", "operation": "update_node", "nodeId": "bbbbbbbbbbbbbbbb", "file": "Gone.md" }))
        .await;
    assert_eq!(out["node"]["file"], "Gone.md");
    assert_eq!(
        out["warnings"],
        json!(["File not found in the vault: Gone.md"])
    );
}

#[tokio::test]
async fn update_refuses_fields_of_another_node_type() {
    let f = setup();
    let e = f
        .edit_err(json!({ "path": "Board.canvas", "operation": "update_node", "nodeId": "bbbbbbbbbbbbbbbb", "text": "x" }))
        .await;
    assert_matches(&e, "`text` does not apply to a file node");
}

#[tokio::test]
async fn update_allows_any_content_field_on_unknown_node_types() {
    let f = setup();
    f.write(
        "Ext.canvas",
        r#"{"nodes":[{"id":"z","type":"custom","x":0,"y":0,"width":1,"height":1}]}"#,
    );
    let out = f
        .edit(json!({ "path": "Ext", "operation": "update_node", "nodeId": "z", "text": "ok" }))
        .await;
    assert_eq!(out["node"]["text"], "ok");
}

#[tokio::test]
async fn update_errors_without_a_node_or_a_change() {
    let f = setup();
    assert_matches(
        &f.edit_err(json!({ "path": "Board.canvas", "operation": "update_node", "x": 1 }))
            .await,
        "needs `nodeId`",
    );
    assert_matches(
        &f.edit_err(
            json!({ "path": "Board.canvas", "operation": "update_node", "nodeId": "nope", "x": 1 }),
        )
        .await,
        r#"No node with id "nope""#,
    );
    assert_matches(
        &f.edit_err(json!({ "path": "Board.canvas", "operation": "update_node", "nodeId": "aaaaaaaaaaaaaaaa" }))
            .await,
        "at least one field",
    );
}

#[tokio::test]
async fn update_errors_on_a_missing_canvas_without_creating_it() {
    let f = setup();
    let e = f
        .edit_err(json!({ "path": "Nope", "operation": "update_node", "nodeId": "a", "x": 1 }))
        .await;
    assert_matches(&e, r"Canvas not found: Nope\.canvas");
    assert!(!f.vault().join("Nope.canvas").exists());
}

// edit_canvas delete_node

#[tokio::test]
async fn delete_removes_the_node_and_its_edges() {
    let f = setup();
    let out = f
        .edit(json!({ "path": "Board.canvas", "operation": "delete_node", "nodeId": "bbbbbbbbbbbbbbbb" }))
        .await;
    assert_eq!(out["node"]["id"], "bbbbbbbbbbbbbbbb");
    assert_eq!(super::ids(&out["removedEdges"]), ["eeeeeeeeeeeeeeee"]);
    let doc = f.doc("Board.canvas");
    assert_eq!(
        super::ids(&doc["nodes"]),
        ["aaaaaaaaaaaaaaaa", "cccccccccccccccc"]
    );
    assert_eq!(doc["edges"], json!([]));
}

#[tokio::test]
async fn delete_errors_on_an_unknown_node() {
    let f = setup();
    assert_matches(
        &f.edit_err(json!({ "path": "Board.canvas", "operation": "delete_node", "nodeId": "x" }))
            .await,
        "No node with id",
    );
}

// edit_canvas connect / disconnect

#[tokio::test]
async fn connect_writes_sides_ends_label_and_color() {
    let f = setup();
    let out = f
        .edit(json!({
            "path": "Board.canvas", "operation": "connect", "fromNode": "cccccccccccccccc", "toNode": "aaaaaaaaaaaaaaaa",
            "fromSide": "top", "toSide": "bottom", "fromEnd": "arrow", "toEnd": "none", "label": "leads to", "color": "2",
        }))
        .await;
    assert_subset(
        &out["edge"],
        json!({
            "fromNode": "cccccccccccccccc", "toNode": "aaaaaaaaaaaaaaaa", "fromSide": "top", "toSide": "bottom",
            "fromEnd": "arrow", "toEnd": "none", "label": "leads to", "color": "2",
        }),
    );
    assert_matches(out["edge"]["id"].as_str().unwrap(), "^[0-9a-f]{16}$");
    assert_eq!(
        f.doc("Board.canvas")["edges"],
        json!([board()["edges"][0], out["edge"]])
    );
}

#[tokio::test]
async fn connect_writes_a_minimal_edge() {
    let f = setup();
    let out = f
        .edit(json!({ "path": "Board.canvas", "operation": "connect", "fromNode": "aaaaaaaaaaaaaaaa", "toNode": "cccccccccccccccc", "label": null }))
        .await;
    let keys: Vec<&String> = out["edge"].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["id", "fromNode", "toNode"]);
}

#[tokio::test]
async fn connect_requires_both_nodes() {
    let f = setup();
    assert_matches(
        &f.edit_err(
            json!({ "path": "Board.canvas", "operation": "connect", "toNode": "aaaaaaaaaaaaaaaa" }),
        )
        .await,
        "needs `fromNode`",
    );
    assert_matches(
        &f.edit_err(json!({ "path": "Board.canvas", "operation": "connect", "fromNode": "aaaaaaaaaaaaaaaa", "toNode": "zz" }))
            .await,
        r#"No node with id "zz""#,
    );
}

#[tokio::test]
async fn connect_refuses_node_fields() {
    let f = setup();
    assert_matches(
        &f.edit_err(json!({ "path": "Board.canvas", "operation": "connect", "fromNode": "a", "toNode": "b", "x": 1 }))
            .await,
        "`x` does not apply to connect",
    );
}

#[tokio::test]
async fn disconnect_removes_an_edge_by_id() {
    let f = setup();
    let out = f
        .edit(json!({ "path": "Board.canvas", "operation": "disconnect", "edgeId": "eeeeeeeeeeeeeeee" }))
        .await;
    assert_eq!(out["edge"], board()["edges"][0]);
    let doc = f.doc("Board.canvas");
    assert_eq!(doc["edges"], json!([]));
    assert_eq!(doc["nodes"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn disconnect_errors_on_a_missing_or_unknown_edge() {
    let f = setup();
    assert_matches(
        &f.edit_err(json!({ "path": "Board.canvas", "operation": "disconnect" }))
            .await,
        "needs `edgeId`",
    );
    assert_matches(
        &f.edit_err(json!({ "path": "Board.canvas", "operation": "disconnect", "edgeId": "x" }))
            .await,
        r#"No edge with id "x""#,
    );
}

#[tokio::test]
async fn leaves_an_invalid_canvas_untouched() {
    let f = setup();
    f.write("Bad.canvas", "not json");
    assert_matches(
        &f.edit_err(json!({ "path": "Bad", "operation": "add_node", "type": "text", "text": "x" }))
            .await,
        "not valid JSON",
    );
    assert_eq!(f.raw("Bad.canvas"), "not json");
}
