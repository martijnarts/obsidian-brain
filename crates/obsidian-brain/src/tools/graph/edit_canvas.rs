//! `edit_canvas`: one structured change to a `.canvas` file per call. Add,
//! update or delete a node, or add or remove an edge. Fields the call does
//! not name stay as they are, including ones this server does not model.
//! Canvases are not part of the note index, so no reindex follows.

use std::path::Path;

use anyhow::{Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value, json};

use crate::mcp::tool::ToolSpec;
use crate::util::js;
use crate::vault::canvas::{
    CanvasDocument, CanvasItem, DEFAULT_NODE_HEIGHT, DEFAULT_NODE_WIDTH, canvas_path,
    content_fields, generate_canvas_id, place_right_of, read_canvas, write_canvas,
};
use crate::vault::vault_path::resolve_vault_path;

const DESCRIPTION: &str = "Change an Obsidian canvas: `add_node` (creates the canvas if missing; placed right of the right-most node by default), `update_node`, `delete_node` (also removes its edges), `connect` or `disconnect`. Returns the changed node or edge.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Operation {
    AddNode,
    UpdateNode,
    DeleteNode,
    Connect,
    Disconnect,
}

impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::AddNode => "add_node",
            Self::UpdateNode => "update_node",
            Self::DeleteNode => "delete_node",
            Self::Connect => "connect",
            Self::Disconnect => "disconnect",
        }
    }

    /// Arguments the operation accepts besides `operation` and `path`.
    fn allowed(self) -> &'static [&'static str] {
        match self {
            Self::AddNode => &[
                "type", "text", "file", "subpath", "url", "label", "color", "x", "y", "width",
                "height",
            ],
            Self::UpdateNode => &[
                "nodeId", "text", "file", "subpath", "url", "label", "color", "x", "y", "width",
                "height",
            ],
            Self::DeleteNode => &["nodeId"],
            Self::Connect => &[
                "fromNode", "toNode", "fromSide", "toSide", "fromEnd", "toEnd", "label", "color",
            ],
            Self::Disconnect => &["edgeId"],
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum NodeType {
    Text,
    File,
    Link,
    Group,
}

impl NodeType {
    fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::File => "file",
            Self::Link => "link",
            Self::Group => "group",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum End {
    None,
    Arrow,
}

/// Every argument but `path` and `operation`, in schema order.
const ARG_ORDER: [&str; 19] = [
    "nodeId", "type", "text", "file", "subpath", "url", "label", "color", "x", "y", "width",
    "height", "fromNode", "toNode", "fromSide", "toSide", "fromEnd", "toEnd", "edgeId",
];
const NODE_FIELDS: [&str; 10] = [
    "text", "file", "subpath", "url", "label", "color", "x", "y", "width", "height",
];
const CONTENT_KEYS: [&str; 5] = ["text", "file", "subpath", "url", "label"];
/// The fields the schema lets a client set to `null`, to remove them.
const NULLABLE: [&str; 3] = ["subpath", "label", "color"];

fn required_content(node_type: &str) -> Option<&'static str> {
    match node_type {
        "text" => Some("text"),
        "file" => Some("file"),
        "link" => Some("url"),
        _ => None,
    }
}

/// A field that may be absent, `null` or a value: `Some(None)` is `null`.
fn nullable<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Ok(Some(Option::deserialize(d)?))
}

/// zod's `.positive()` on a number.
fn opt_positive_number<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    use serde::de::Error;
    match Option::<f64>::deserialize(d)? {
        Some(x) if x <= 0.0 => Err(D::Error::custom(format!(
            "expected a positive number, got {}",
            js::number_to_string(x)
        ))),
        other => Ok(other),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Vault-relative path of the `.canvas` file. The extension is optional.
    #[serde(deserialize_with = "super::non_empty")]
    #[schemars(length(min = 1))]
    path: String,
    operation: Operation,
    /// Node to change (`update_node`, `delete_node`).
    node_id: Option<String>,
    /// Node type (`add_node`).
    #[serde(rename = "type")]
    node_type: Option<NodeType>,
    /// Markdown content of a `text` node.
    text: Option<String>,
    /// Vault-relative file a `file` node shows.
    file: Option<String>,
    /// Heading or block of a `file` node, e.g. `#Heading`. `null` removes it.
    #[serde(default, deserialize_with = "nullable")]
    subpath: Option<Option<String>>,
    /// URL of a `link` node.
    url: Option<String>,
    /// Label of a `group` node or an edge. `null` removes it.
    #[serde(default, deserialize_with = "nullable")]
    label: Option<Option<String>>,
    #[schemars(
        description = "Preset `\"1\"`-`\"6\"` or hex like `\"#ff0000\"`. `null` removes it."
    )]
    #[serde(default, deserialize_with = "nullable")]
    color: Option<Option<String>>,
    x: Option<f64>,
    y: Option<f64>,
    /// Default 400.
    #[serde(default, deserialize_with = "opt_positive_number")]
    #[schemars(extend("exclusiveMinimum" = 0))]
    width: Option<f64>,
    /// Default 200.
    #[serde(default, deserialize_with = "opt_positive_number")]
    #[schemars(extend("exclusiveMinimum" = 0))]
    height: Option<f64>,
    /// Edge start node id (`connect`).
    from_node: Option<String>,
    /// Edge end node id (`connect`).
    to_node: Option<String>,
    from_side: Option<Side>,
    to_side: Option<Side>,
    /// Default `none`.
    from_end: Option<End>,
    /// Default `arrow`.
    to_end: Option<End>,
    /// Edge to remove (`disconnect`).
    edge_id: Option<String>,
}

fn enum_value<T: Serialize>(v: &Option<T>) -> Option<Value> {
    v.as_ref()
        .map(|v| serde_json::to_value(v).expect("enums serialize"))
}

fn str_value(v: &Option<String>) -> Option<Value> {
    v.clone().map(Value::String)
}

impl Args {
    /// JS `args[key]`: `None` when absent, `Some(Null)` for an explicit null.
    fn get(&self, key: &str) -> Option<Value> {
        match key {
            "nodeId" => str_value(&self.node_id),
            "type" => self.node_type.map(|t| t.name().into()),
            "text" => str_value(&self.text),
            "file" => str_value(&self.file),
            "subpath" => self
                .subpath
                .clone()
                .map(|v| v.map_or(Value::Null, Value::String)),
            "url" => str_value(&self.url),
            "label" => self
                .label
                .clone()
                .map(|v| v.map_or(Value::Null, Value::String)),
            "color" => self
                .color
                .clone()
                .map(|v| v.map_or(Value::Null, Value::String)),
            "x" => self.x.map(js::number_value),
            "y" => self.y.map(js::number_value),
            "width" => self.width.map(js::number_value),
            "height" => self.height.map(js::number_value),
            "fromNode" => str_value(&self.from_node),
            "toNode" => str_value(&self.to_node),
            "fromSide" => enum_value(&self.from_side),
            "toSide" => enum_value(&self.to_side),
            "fromEnd" => enum_value(&self.from_end),
            "toEnd" => enum_value(&self.to_end),
            "edgeId" => str_value(&self.edge_id),
            _ => None,
        }
    }
}

fn id_value(item: &CanvasItem) -> Value {
    item.get("id").cloned().unwrap_or(Value::Null)
}

/// The index of the node with id `id`.
fn find_node(
    doc: &CanvasDocument,
    id: Option<&str>,
    operation: &str,
    arg_name: &str,
) -> Result<usize> {
    let Some(id) = id.filter(|id| !id.is_empty()) else {
        bail!("{operation} needs `{arg_name}`");
    };
    match doc
        .nodes
        .iter()
        .position(|n| n.get("id").and_then(Value::as_str) == Some(id))
    {
        Some(i) => Ok(i),
        None => bail!("No node with id \"{id}\""),
    }
}

/// Refuse content fields that belong to another node type, and removing
/// required ones.
fn check_content_fields(node_type: &str, args: &Args) -> Result<()> {
    let Some(own) = content_fields(node_type) else {
        return Ok(());
    };
    for key in CONTENT_KEYS {
        let Some(value) = args.get(key) else { continue };
        if !own.contains(&key) {
            bail!("`{key}` does not apply to a {node_type} node");
        }
        if value.is_null() && required_content(node_type) == Some(key) {
            bail!("`{key}` is required on a {node_type} node and cannot be removed");
        }
    }
    Ok(())
}

/// The target must stay inside the vault; a missing target only warns.
fn check_file_target(vault: &Path, file: &str, warnings: &mut Vec<String>) -> Result<()> {
    let abs = resolve_vault_path(vault, file)?.abs;
    if !abs.exists() {
        warnings.push(format!("File not found in the vault: {file}"));
    }
    Ok(())
}

fn edit(vault: &Path, args: &Args) -> Result<Value> {
    let operation = args.operation;
    let path = canvas_path(&args.path);
    let given: Vec<&str> = ARG_ORDER
        .into_iter()
        .filter(|k| args.get(k).is_some())
        .collect();
    let stray: Vec<String> = given
        .iter()
        .filter(|k| !operation.allowed().contains(k))
        .map(|k| format!("`{k}`"))
        .collect();
    if !stray.is_empty() {
        bail!(
            "{} does not apply to {}",
            stray.join(", "),
            operation.name()
        );
    }

    let existing = read_canvas(vault, &path)?;
    if existing.is_none() && operation != Operation::AddNode {
        bail!("Canvas not found: {path}");
    }
    let created = existing.is_none();
    let mut doc = existing.unwrap_or_default();
    let mut warnings: Vec<String> = Vec::new();
    let mut result = Map::new();

    match operation {
        Operation::AddNode => {
            let Some(node_type) = args.node_type else {
                bail!("add_node needs `type`");
            };
            let ty = node_type.name();
            check_content_fields(ty, args)?;
            if let Some(required) = required_content(ty)
                && args.get(required).is_none()
            {
                bail!("A {ty} node needs `{required}`");
            }
            let (px, py) = place_right_of(&doc.nodes);
            let mut node = CanvasItem::new();
            node.insert("id".into(), generate_canvas_id(&doc).into());
            node.insert("type".into(), ty.into());
            node.insert("x".into(), js::number_value(args.x.unwrap_or(px)));
            node.insert("y".into(), js::number_value(args.y.unwrap_or(py)));
            node.insert(
                "width".into(),
                js::number_value(args.width.unwrap_or(DEFAULT_NODE_WIDTH)),
            );
            node.insert(
                "height".into(),
                js::number_value(args.height.unwrap_or(DEFAULT_NODE_HEIGHT)),
            );
            for key in CONTENT_KEYS.into_iter().chain(["color"]) {
                if let Some(value) = args.get(key).filter(|v| !v.is_null()) {
                    node.insert(key.into(), value);
                }
            }
            if let Some(Value::String(file)) = node.get("file") {
                check_file_target(vault, file, &mut warnings)?;
            }
            doc.nodes.push(node.clone());
            result.insert("node".into(), Value::Object(node));
            result.insert("created".into(), created.into());
        }

        Operation::UpdateNode => {
            let i = find_node(&doc, args.node_id.as_deref(), "update_node", "nodeId")?;
            if !given.iter().any(|k| *k != "nodeId") {
                bail!("update_node needs at least one field to change");
            }
            if let Some(ty) = doc.nodes[i].get("type").and_then(Value::as_str) {
                check_content_fields(ty, args)?;
            }
            let node = &mut doc.nodes[i];
            for key in NODE_FIELDS {
                match args.get(key) {
                    None => {}
                    Some(Value::Null) => {
                        node.shift_remove(key);
                    }
                    Some(value) => {
                        node.insert(key.into(), value);
                    }
                }
            }
            let node = node.clone();
            if let Some(file) = &args.file {
                check_file_target(vault, file, &mut warnings)?;
            }
            result.insert("node".into(), Value::Object(node));
        }

        Operation::DeleteNode => {
            let i = find_node(&doc, args.node_id.as_deref(), "delete_node", "nodeId")?;
            let node = doc.nodes.remove(i);
            let id = id_value(&node);
            let touches =
                |e: &CanvasItem| e.get("fromNode") == Some(&id) || e.get("toNode") == Some(&id);
            let (removed, kept): (Vec<CanvasItem>, Vec<CanvasItem>) =
                std::mem::take(&mut doc.edges)
                    .into_iter()
                    .partition(touches);
            doc.edges = kept;
            result.insert("node".into(), Value::Object(node));
            result.insert(
                "removedEdges".into(),
                Value::Array(removed.into_iter().map(Value::Object).collect()),
            );
        }

        Operation::Connect => {
            let from = find_node(&doc, args.from_node.as_deref(), "connect", "fromNode")?;
            let to = find_node(&doc, args.to_node.as_deref(), "connect", "toNode")?;
            let mut edge = CanvasItem::new();
            edge.insert("id".into(), generate_canvas_id(&doc).into());
            edge.insert("fromNode".into(), id_value(&doc.nodes[from]));
            edge.insert("toNode".into(), id_value(&doc.nodes[to]));
            for key in ["fromSide", "toSide", "fromEnd", "toEnd", "color", "label"] {
                if let Some(value) = args.get(key).filter(|v| !v.is_null()) {
                    edge.insert(key.into(), value);
                }
            }
            doc.edges.push(edge.clone());
            result.insert("edge".into(), Value::Object(edge));
        }

        Operation::Disconnect => {
            let Some(edge_id) = args.edge_id.as_deref().filter(|id| !id.is_empty()) else {
                bail!("disconnect needs `edgeId`");
            };
            let Some(i) = doc
                .edges
                .iter()
                .position(|e| e.get("id").and_then(Value::as_str) == Some(edge_id))
            else {
                bail!("No edge with id \"{edge_id}\" in {path}");
            };
            let edge = doc.edges.remove(i);
            result.insert("edge".into(), Value::Object(edge));
        }
    }

    write_canvas(vault, &path, &doc)?;
    let mut out = Map::new();
    out.insert("path".into(), path.into());
    out.insert("operation".into(), operation.name().into());
    out.extend(result);
    if !warnings.is_empty() {
        out.insert("warnings".into(), json!(warnings));
    }
    Ok(super::js_numbers(Value::Object(out)))
}

pub fn spec() -> ToolSpec {
    let mut spec =
        ToolSpec::new::<Args, _, _>("edit_canvas", DESCRIPTION, |ctx, args| async move {
            edit(&ctx.config.vault_path, &args)
        });
    // `null` removes these fields, so the schema allows it; the shared
    // schema code drops the `null` schemars gives every `Option`.
    if let Some(Value::Object(props)) = spec.input_schema.get_mut("properties") {
        for key in NULLABLE {
            if let Some(Value::Object(prop)) = props.get_mut(key) {
                prop.insert("type".into(), json!(["string", "null"]));
            }
        }
    }
    spec
}
