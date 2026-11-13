//! `query_notes`: filter notes by metadata with a JsonLogic expression.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::Path;
use std::time::UNIX_EPOCH;

use anyhow::bail;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::json_logic::{self, JsVal};
use super::list_notes::iso_from_ms;
use crate::mcp::tool::ToolSpec;
use crate::types::Frontmatter;
use crate::util::js;

const DESCRIPTION: &str = "Filter notes by metadata with a JsonLogic `filter`; the headless replacement for Dataview queries. Each note is `{path, folder, name, title, tags, frontmatter, mtime, size}`: `tags` merges frontmatter and inline tags without `#`, `mtime` is ISO 8601, and `{\"var\": \"frontmatter.status\"}` reads a property. The extra operator `has_tag` matches a tag case-insensitively, nested tags included. Example: `{\"and\": [{\"has_tag\": [{\"var\": \"tags\"}, \"book\"]}, {\">=\": [{\"var\": \"frontmatter.rating\"}, 4]}]}`.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Order {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct Sort {
    /// `path`, `title`, `mtime` or `frontmatter.<key>`.
    field: String,
    /// Default `asc`.
    order: Option<Order>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct Args {
    /// JsonLogic expression; a note matches when it is truthy. `{"==": [1, 1]}` matches every note.
    #[schemars(with = "Map<String, Value>")]
    filter: Value,
    /// Frontmatter keys to return. Default all.
    fields: Option<Vec<String>>,
    /// Default: by path, ascending.
    sort: Option<Sort>,
    /// Max results (1-500). Default 50.
    #[schemars(range(min = 1, max = 500))]
    limit: Option<u64>,
}

/// The record a `filter` sees for each note.
#[derive(Debug, Clone)]
struct NoteRecord {
    path: String,
    folder: String,
    name: String,
    title: String,
    tags: Vec<String>,
    frontmatter: Frontmatter,
    mtime: String,
    size: u64,
}

impl NoteRecord {
    fn to_value(&self) -> Value {
        json!({
            "path": self.path,
            "folder": self.folder,
            "name": self.name,
            "title": self.title,
            "tags": self.tags,
            "frontmatter": self.frontmatter,
            "mtime": self.mtime,
            "size": self.size,
        })
    }
}

/// Frontmatter `tags` (or `tag`) plus the body's inline tags, without `#`,
/// deduplicated case-insensitively. A frontmatter string is split on commas
/// and whitespace, as Obsidian reads it.
pub fn note_tags(fm: &Frontmatter) -> Vec<String> {
    let mut raw: Vec<String> = Vec::new();
    for key in ["tags", "tag", "inline_tags"] {
        match fm.get(key) {
            Some(Value::Array(items)) => raw.extend(items.iter().filter_map(|v| match v {
                Value::String(s) => Some(s.clone()),
                _ => None,
            })),
            Some(Value::String(s)) => raw.extend(
                s.split(|c: char| c == ',' || js::is_js_whitespace(c))
                    .map(str::to_string),
            ),
            _ => {}
        }
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for t in raw {
        let t = js::trim(&t);
        let tag = t.strip_prefix('#').unwrap_or(t);
        if tag.is_empty() || !seen.insert(tag.to_lowercase()) {
            continue;
        }
        out.push(tag.to_string());
    }
    out
}

const FILTER_ERROR: &str = "filter must be a JsonLogic object with exactly one operator, e.g. {\"==\": [{\"var\": \"frontmatter.status\"}, \"active\"]}";

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("query_notes", DESCRIPTION, |ctx, args| async move {
        if !json_logic::is_logic(&args.filter) {
            bail!(FILTER_ERROR);
        }
        let sort_field = args
            .sort
            .as_ref()
            .map_or("path".to_string(), |s| s.field.clone());
        if !valid_sort_field(&sort_field) {
            bail!(
                "sort.field must be path, title, mtime or frontmatter.<key>, got \"{sort_field}\""
            );
        }
        let rows: Vec<(String, String, String)> = ctx
            .db
            .run(|c| -> rusqlite::Result<Vec<(String, String, String)>> {
                let mut stmt = c.prepare("SELECT id, title, frontmatter FROM nodes")?;
                stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect()
            })
            .await?;
        let root = ctx.config.vault_path.clone();
        tokio::task::spawn_blocking(move || query(&root, rows, &args, &sort_field)).await?
    })
}

/// `path`, `title`, `mtime`, or what `/^frontmatter\..+/` accepts.
fn valid_sort_field(field: &str) -> bool {
    if matches!(field, "path" | "title" | "mtime") {
        return true;
    }
    field
        .strip_prefix("frontmatter.")
        .and_then(|rest| rest.chars().next())
        .is_some_and(|c| !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
}

fn query(
    root: &Path,
    rows: Vec<(String, String, String)>,
    args: &Args,
    sort_field: &str,
) -> anyhow::Result<Value> {
    let desc = args.sort.as_ref().and_then(|s| s.order) == Some(Order::Desc);
    let limit = args.limit.unwrap_or(50) as usize;

    let mut matched: Vec<NoteRecord> = Vec::new();
    for (id, title, fm) in rows {
        if id.starts_with("_stub/") {
            continue;
        }
        let parsed: Frontmatter = serde_json::from_str(&fm)?;
        if parsed.get("_stub") == Some(&Value::Bool(true)) {
            continue;
        }
        let Ok(st) = std::fs::metadata(root.join(&id)) else {
            continue; // deleted on disk, index not caught up yet
        };
        let mut frontmatter = parsed.clone();
        frontmatter.shift_remove("inline_tags");
        let (folder, file) = match id.rfind('/') {
            Some(i) => (id[..i].to_string(), &id[i + 1..]),
            None => (String::new(), id.as_str()),
        };
        let name = file.strip_suffix(".md").unwrap_or(file).to_string();
        let rec = NoteRecord {
            folder,
            name,
            title,
            tags: note_tags(&parsed),
            frontmatter,
            mtime: mtime_iso(&st).unwrap_or_default(),
            size: st.len(),
            path: id,
        };
        let hit = json_logic::apply(&args.filter, &JsVal::from(&rec.to_value()))
            .map_err(|e| anyhow::anyhow!("Invalid filter: {e}"))?;
        if json_logic::truthy(&hit) {
            matched.push(rec);
        }
    }

    matched.sort_by(|a, b| {
        let by_path = || js::locale_compare(&a.path, &b.path);
        let (va, vb) = (sort_value(a, sort_field), sort_value(b, sort_field));
        let missing = |v: &Option<Value>| matches!(v, None | Some(Value::Null));
        match (missing(&va), missing(&vb)) {
            (true, true) => return by_path(),
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
            _ => {}
        }
        let (va, vb) = (va.expect("present"), vb.expect("present"));
        let c = compare_values(&va, &vb);
        let c = if desc { c.reverse() } else { c };
        c.then_with(by_path)
    });

    let total = matched.len();
    let results: Vec<Value> = matched
        .iter()
        .take(limit)
        .map(|rec| {
            let frontmatter = match &args.fields {
                Some(fields) => {
                    let mut picked = Map::new();
                    for k in fields {
                        if let Some(v) = rec.frontmatter.get(k) {
                            picked.insert(k.clone(), v.clone());
                        }
                    }
                    js::js_object_order(&mut picked);
                    picked
                }
                None => rec.frontmatter.clone(),
            };
            json!({
                "path": rec.path,
                "title": rec.title,
                "tags": rec.tags,
                "mtime": rec.mtime,
                "size": rec.size,
                "frontmatter": frontmatter,
            })
        })
        .collect();
    let mut out = Map::new();
    out.insert("total".into(), total.into());
    if total > limit {
        out.insert("truncated".into(), true.into());
    }
    out.insert("results".into(), Value::Array(results));
    Ok(Value::Object(out))
}

/// `stat.mtime.toISOString()`.
fn mtime_iso(st: &std::fs::Metadata) -> Option<String> {
    let modified = st.modified().ok()?;
    let ms = match modified.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64() * 1000.0,
        Err(e) => -(e.duration().as_secs_f64() * 1000.0),
    };
    iso_from_ms(ms)
}

fn sort_value(rec: &NoteRecord, field: &str) -> Option<Value> {
    match field {
        "path" => Some(rec.path.clone().into()),
        "title" => Some(rec.title.clone().into()),
        "mtime" => Some(rec.mtime.clone().into()),
        _ => rec.frontmatter.get(&field["frontmatter.".len()..]).cloned(),
    }
}

/// Numbers compare numerically, anything else as text.
fn compare_values(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => js::as_f64(x)
            .partial_cmp(&js::as_f64(y))
            .unwrap_or(Ordering::Equal),
        _ => js::locale_compare(&js::to_js_string(a), &js::to_js_string(b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fm(v: Value) -> Frontmatter {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn merges_frontmatter_and_inline_tags() {
        assert_eq!(
            note_tags(&fm(
                json!({"tags": ["Book", "#fiction"], "inline_tags": ["book", "todo"]})
            )),
            ["Book", "fiction", "todo"]
        );
    }

    #[test]
    fn splits_a_string_and_reads_the_tag_key() {
        assert_eq!(note_tags(&fm(json!({"tags": "a, b c"}))), ["a", "b", "c"]);
        assert_eq!(note_tags(&fm(json!({"tag": "solo"}))), ["solo"]);
        assert!(note_tags(&fm(json!({"tags": [1, null, ""]}))).is_empty());
        assert_eq!(
            note_tags(&fm(
                json!({"tags": [" #x "], "tag": "y", "inline_tags": ["X"]})
            )),
            ["x", "y"]
        );
    }

    #[test]
    fn sort_fields_follow_the_ts_pattern() {
        for ok in ["path", "title", "mtime", "frontmatter.a", "frontmatter.a.b"] {
            assert!(valid_sort_field(ok), "{ok}");
        }
        for bad in ["size", "frontmatter.", "frontmatter.\nx", "Path"] {
            assert!(!valid_sort_field(bad), "{bad}");
        }
    }
}
