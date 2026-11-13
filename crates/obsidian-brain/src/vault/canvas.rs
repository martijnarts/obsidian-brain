//! Read and write `.canvas` files (JSON Canvas 1.0, https://jsoncanvas.org).
//!
//! Nodes and edges are kept as plain JSON objects: a write changes only the
//! fields a tool touches, so styling, plugin data and future spec fields
//! survive a round trip.

use std::io;
use std::path::Path;

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value};

use super::vault_path::{read_utf8, resolve_vault_path, write_file_atomic};
use crate::util::js::json_stringify;

/// One node or edge: `id`, `type`, `x`, `y`, `width`, `height` and the rest
/// of the JSON Canvas fields, in file order.
pub type CanvasItem = Map<String, Value>;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CanvasDocument {
    pub nodes: Vec<CanvasItem>,
    pub edges: Vec<CanvasItem>,
    /// Every other top-level key, in file order.
    pub rest: Map<String, Value>,
}

/// Content fields each node type carries, per the JSON Canvas spec.
pub fn content_fields(node_type: &str) -> Option<&'static [&'static str]> {
    Some(match node_type {
        "text" => &["text"],
        "file" => &["file", "subpath"],
        "link" => &["url"],
        "group" => &["label"],
        _ => return None,
    })
}

pub const DEFAULT_NODE_WIDTH: f64 = 400.0;
pub const DEFAULT_NODE_HEIGHT: f64 = 200.0;
const PLACEMENT_GAP: f64 = 40.0;

/// `path` with `.canvas` appended when the caller left it off.
pub fn canvas_path(path: &str) -> String {
    if path.ends_with(".canvas") {
        path.to_string()
    } else {
        format!("{path}.canvas")
    }
}

/// Parse canvas JSON. An empty file is an empty canvas: Obsidian creates new
/// canvases as zero-byte files. Missing `nodes` / `edges` keys read as empty.
pub fn parse_canvas(raw: &str, rel_path: &str) -> Result<CanvasDocument> {
    if crate::util::js::trim(raw).is_empty() {
        return Ok(CanvasDocument::default());
    }
    let data: Value = serde_json::from_str(raw)
        .map_err(|e| anyhow!("Canvas {rel_path} is not valid JSON: {e}"))?;
    let Value::Object(mut doc) = data else {
        bail!("Canvas {rel_path} is not a JSON object");
    };
    let mut take = |key: &str| -> Result<Vec<CanvasItem>> {
        match doc.shift_remove(key) {
            None => Ok(Vec::new()),
            Some(Value::Array(items)) => Ok(items
                .into_iter()
                .map(|item| match item {
                    Value::Object(m) => m,
                    // A non-object entry is no valid node or edge; it reads
                    // as an empty object.
                    _ => Map::new(),
                })
                .collect()),
            Some(_) => bail!("Canvas {rel_path} has a non-array \"{key}\" field"),
        }
    };
    let nodes = take("nodes")?;
    let edges = take("edges")?;
    Ok(CanvasDocument {
        nodes,
        edges,
        rest: doc,
    })
}

/// Serialize the way Obsidian does: tab-indented top level, one compact JSON
/// object per node and edge. Keeps diffs of synced vaults to one line per
/// changed element.
pub fn serialize_canvas(doc: &CanvasDocument) -> String {
    let mut entries = vec![
        format!("\t\"nodes\":{}", serialize_array(&doc.nodes)),
        format!("\t\"edges\":{}", serialize_array(&doc.edges)),
    ];
    for (key, value) in &doc.rest {
        entries.push(format!(
            "\t{}:{}",
            json_stringify(&Value::String(key.clone())),
            json_stringify(value)
        ));
    }
    format!("{{\n{}\n}}", entries.join(",\n"))
}

fn serialize_array(items: &[CanvasItem]) -> String {
    if items.is_empty() {
        return "[]".into();
    }
    let lines: Vec<String> = items
        .iter()
        .map(|item| format!("\t\t{}", json_stringify(&Value::Object(item.clone()))))
        .collect();
    format!("[\n{}\n\t]", lines.join(",\n"))
}

/// Read a canvas; `None` when the file does not exist.
pub fn read_canvas(vault_path: &Path, rel_path: &str) -> Result<Option<CanvasDocument>> {
    let abs = resolve_vault_path(vault_path, rel_path)?.abs;
    let raw = match read_utf8(&abs) {
        Ok(raw) => raw,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(anyhow!(crate::util::errors::node_fs_error_message(
                &e, "open", &abs, None
            )));
        }
    };
    parse_canvas(&raw, rel_path).map(Some)
}

/// Write a canvas atomically (temp file + rename), creating parent folders.
pub fn write_canvas(vault_path: &Path, rel_path: &str, doc: &CanvasDocument) -> Result<()> {
    let abs = resolve_vault_path(vault_path, rel_path)?.abs;
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_file_atomic(&abs, serialize_canvas(doc))?;
    Ok(())
}

fn id_of(item: &CanvasItem) -> Option<&str> {
    item.get("id").and_then(Value::as_str)
}

/// A 16-hex-char id, unique among `doc`'s nodes and edges, like Obsidian's.
pub fn generate_canvas_id(doc: &CanvasDocument) -> String {
    loop {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).expect("the OS random source works");
        let id = hex::encode(bytes);
        if !doc
            .nodes
            .iter()
            .chain(&doc.edges)
            .any(|item| id_of(item) == Some(&id))
        {
            return id;
        }
    }
}

/// Top-left corner for a new node: right of the right-most node, aligned to its top.
pub fn place_right_of(nodes: &[CanvasItem]) -> (f64, f64) {
    let mut best: Option<(f64, f64)> = None;
    for node in nodes {
        let right = num(node.get("x")) + num(node.get("width"));
        if best.is_none_or(|(r, _)| right > r) {
            best = Some((right, num(node.get("y"))));
        }
    }
    match best {
        None => (0.0, 0.0),
        Some((right, y)) => (right + PLACEMENT_GAP, y),
    }
}

fn num(value: Option<&Value>) -> f64 {
    value
        .and_then(Value::as_f64)
        .filter(|x| x.is_finite())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_empty_and_partial_canvases() {
        assert_eq!(
            parse_canvas("  \n", "a.canvas").unwrap(),
            CanvasDocument::default()
        );
        let doc = parse_canvas(r#"{"nodes":[{"id":"a"}],"extra":1}"#, "a.canvas").unwrap();
        assert_eq!(doc.nodes.len(), 1);
        assert!(doc.edges.is_empty());
        assert_eq!(doc.rest.get("extra"), Some(&json!(1)));
    }

    #[test]
    fn refuses_bad_canvases() {
        assert!(
            parse_canvas("{", "a.canvas")
                .unwrap_err()
                .to_string()
                .starts_with("Canvas a.canvas is not valid JSON: ")
        );
        assert_eq!(
            parse_canvas("[]", "a.canvas").unwrap_err().to_string(),
            "Canvas a.canvas is not a JSON object"
        );
        assert_eq!(
            parse_canvas(r#"{"edges":{}}"#, "a.canvas")
                .unwrap_err()
                .to_string(),
            "Canvas a.canvas has a non-array \"edges\" field"
        );
    }

    #[test]
    fn serializes_like_obsidian() {
        let doc = parse_canvas(
            r#"{"meta":{"v":1},"nodes":[{"id":"a","type":"text","x":0,"y":0,"width":400.0,"height":200,"text":"hi"}],"edges":[]}"#,
            "a.canvas",
        )
        .unwrap();
        assert_eq!(
            serialize_canvas(&doc),
            "{\n\t\"nodes\":[\n\t\t{\"id\":\"a\",\"type\":\"text\",\"x\":0,\"y\":0,\"width\":400,\"height\":200,\"text\":\"hi\"}\n\t],\n\t\"edges\":[],\n\t\"meta\":{\"v\":1}\n}"
        );
    }

    #[test]
    fn places_new_nodes_to_the_right() {
        assert_eq!(place_right_of(&[]), (0.0, 0.0));
        let nodes: Vec<CanvasItem> = [
            json!({"x": 0, "y": 5, "width": 100}),
            json!({"x": 300, "y": -20, "width": 50}),
            json!({"x": "bad"}),
        ]
        .into_iter()
        .map(|v| v.as_object().unwrap().clone())
        .collect();
        assert_eq!(place_right_of(&nodes), (390.0, -20.0));
    }

    #[test]
    fn generates_unique_ids_and_round_trips_on_disk() {
        let vault = tempfile::tempdir().unwrap();
        assert_eq!(read_canvas(vault.path(), "x.canvas").unwrap(), None);
        let mut doc = CanvasDocument::default();
        let id = generate_canvas_id(&doc);
        assert_eq!(id.len(), 16);
        doc.nodes
            .push(json!({"id": id}).as_object().unwrap().clone());
        write_canvas(vault.path(), "sub/x.canvas", &doc).unwrap();
        assert_eq!(
            read_canvas(vault.path(), "sub/x.canvas").unwrap(),
            Some(doc)
        );
        assert_eq!(canvas_path("a"), "a.canvas");
        assert_eq!(canvas_path("a.canvas"), "a.canvas");
        assert_eq!(content_fields("file"), Some(&["file", "subpath"][..]));
    }
}
