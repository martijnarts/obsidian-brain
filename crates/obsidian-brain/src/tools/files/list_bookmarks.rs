//! `list_bookmarks`: the vault's Obsidian bookmarks from
//! `.obsidian/bookmarks.json`, as a tree.

use std::collections::HashSet;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::mcp::tool::ToolSpec;
use crate::vault::vault_path::read_utf8;

const DESCRIPTION: &str = "List the vault's Obsidian bookmarks (`.obsidian/bookmarks.json`) as a tree of groups, files, folders, searches, headings and blocks. `totalItems` counts every non-group bookmark.";

const BOOKMARK_TYPES: [&str; 6] = ["file", "folder", "search", "heading", "block", "group"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum BookmarkType {
    File,
    Folder,
    Search,
    Heading,
    Block,
    Group,
}

impl BookmarkType {
    fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Folder => "folder",
            Self::Search => "search",
            Self::Heading => "heading",
            Self::Block => "block",
            Self::Group => "group",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct Args {
    /// Only these bookmark types. Without `group`, group contents are flattened.
    types: Option<Vec<BookmarkType>>,
}

/// One bookmark as a JSON object: `type`, then the known string fields,
/// then `items` for a group.
type Bookmark = Map<String, Value>;

fn kind(b: &Bookmark) -> &str {
    b.get("type").and_then(Value::as_str).unwrap_or("")
}

fn children(b: &Bookmark) -> Vec<Bookmark> {
    match b.get("items") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.as_object().cloned())
            .collect(),
        _ => Vec::new(),
    }
}

/// Keeps the known fields of one raw entry; drops entries of unknown type.
fn to_bookmark(raw: &Value) -> Option<Bookmark> {
    let r = raw.as_object()?;
    let t = r.get("type").and_then(Value::as_str)?;
    if !BOOKMARK_TYPES.contains(&t) {
        return None;
    }
    let mut b = Bookmark::new();
    b.insert("type".into(), t.into());
    for key in ["title", "path", "query", "subpath"] {
        if let Some(Value::String(s)) = r.get(key) {
            b.insert(key.into(), s.clone().into());
        }
    }
    if t == "group" {
        let items = to_bookmarks(r.get("items").unwrap_or(&Value::Null));
        b.insert(
            "items".into(),
            items.into_iter().map(Value::Object).collect(),
        );
    }
    Some(b)
}

fn to_bookmarks(raw: &Value) -> Vec<Bookmark> {
    match raw {
        Value::Array(items) => items.iter().filter_map(to_bookmark).collect(),
        _ => Vec::new(),
    }
}

/// Without `group` in `types`, a group's matching children move up a level.
fn filter_bookmarks(items: Vec<Bookmark>, types: &HashSet<&str>) -> Vec<Bookmark> {
    let mut out = Vec::new();
    for mut b in items {
        if kind(&b) == "group" {
            let kept = filter_bookmarks(children(&b), types);
            if types.contains("group") {
                b.insert(
                    "items".into(),
                    kept.into_iter().map(Value::Object).collect(),
                );
                out.push(b);
            } else {
                out.extend(kept);
            }
        } else if types.contains(kind(&b)) {
            out.push(b);
        }
    }
    out
}

fn count_bookmarks(items: &[Bookmark]) -> usize {
    items
        .iter()
        .map(|b| {
            if kind(b) == "group" {
                count_bookmarks(&children(b))
            } else {
                1
            }
        })
        .sum()
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("list_bookmarks", DESCRIPTION, |ctx, args| async move {
        let path = ctx
            .config
            .vault_path
            .join(".obsidian")
            .join("bookmarks.json");
        let Ok(raw) = read_utf8(&path) else {
            return Ok(
                json!({ "totalItems": 0, "items": [], "note": "No bookmarks file in this vault." }),
            );
        };

        let parsed: Option<Value> = serde_json::from_str(&raw).ok();
        let Some(raw_items @ Value::Array(_)) = parsed.as_ref().and_then(|p| p.get("items")) else {
            return Ok(json!({
                "totalItems": 0,
                "items": [],
                "note": "The bookmarks file is not valid bookmarks JSON.",
            }));
        };
        let mut items = to_bookmarks(raw_items);

        if let Some(types) = args.types.filter(|t| !t.is_empty()) {
            let types: HashSet<&str> = types.iter().map(|t| t.as_str()).collect();
            items = filter_bookmarks(items, &types);
        }
        Ok(json!({
            "totalItems": count_bookmarks(&items),
            "items": items,
        }))
    })
}
