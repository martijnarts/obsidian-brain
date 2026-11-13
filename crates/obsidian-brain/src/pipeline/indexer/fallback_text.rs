//! Searchable text for a note whose body yields no chunks.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::types::ParsedNode;
use crate::util::js;

/// A fallback text shorter than this (UTF-16 units, trimmed) is no
/// identity to embed; the note is recorded as `no-embeddable-content`.
pub const MIN_FALLBACK_CHARS: usize = 3;

static LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!?\[\[([^\]|#^]+)").expect("valid pattern"));

/// The embedding text for a note whose body produced no chunks (an empty
/// file, frontmatter only, embeds only, a body under `min_chunk_chars`):
/// the title, the tags, the scalar frontmatter values and the first 5
/// wiki-link or embed targets, one per line. `""` when there is nothing;
/// the caller records that as `no-embeddable-content`.
pub fn build_title_fallback_text(node: &ParsedNode) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !node.title.is_empty() {
        parts.push(node.title.clone());
    }

    let tags: Vec<&str> = match node.frontmatter.get("tags") {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    if !tags.is_empty() {
        parts.push(tags.join(", "));
    }

    for (key, val) in &node.frontmatter {
        if key == "tags" || key.starts_with('_') {
            continue;
        }
        match val {
            Value::String(s) if !js::trim(s).is_empty() => {
                parts.push(format!("{key}: {}", js::trim(s)));
            }
            Value::Number(_) | Value::Bool(_) => {
                parts.push(format!("{key}: {}", js::to_js_string(val)));
            }
            _ => {}
        }
    }

    // Embeds-only collector notes (`![[a.png]] ![[b.png]]`) still get
    // searchable text from their targets.
    let mut links: Vec<&str> = Vec::new();
    for c in LINK.captures_iter(&node.content) {
        if links.len() >= 5 {
            break;
        }
        let target = js::trim(c.get(1).map_or("", |m| m.as_str()));
        if !target.is_empty() {
            links.push(target);
        }
    }
    if !links.is_empty() {
        parts.push(format!("Links: {}", links.join(", ")));
    }

    parts.join("\n")
}
