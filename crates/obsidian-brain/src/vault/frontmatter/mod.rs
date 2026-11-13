//! YAML frontmatter: split a note the way gray-matter does, parse the block
//! with js-yaml's results, and edit keys in place.
//!
//! [`edit_frontmatter`] changes only the lines of the keys it sets or
//! removes; comments, quoting, key order and list style of every other
//! line stay byte-identical. New lines are written in js-yaml's
//! style. When an edit cannot be made by line (a flow-style block, a merged
//! key, tab indentation), it falls back to re-serialising the whole block,
//! and says so in [`Edited::in_place`].

mod dump;
mod edit;
mod load;

use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

pub use dump::dump_yaml;
pub use edit::{Edited, FrontmatterOp, edit_frontmatter};

use crate::types::Frontmatter;
use crate::util::js;

/// The frontmatter block could not be parsed. The message is the YAML
/// parser's.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct FrontmatterError(pub String);

/// Where a note's frontmatter block sits, in byte offsets of the raw text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// The language after the opening `---` (`yaml`, `json`), or `""`.
    pub language: String,
    /// The text gray-matter hands the YAML parser: from after the opening
    /// `---` (and language) up to the `\n` before the closing `---`.
    pub matter: Range<usize>,
    /// The offset of the `\n` that precedes the closing `---`, or `None`
    /// when the block never closes and runs to the end of the note.
    pub close: Option<usize>,
}

/// A note split into its frontmatter block and body, as gray-matter splits it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split<'a> {
    /// The byte length of a leading BOM, which gray-matter strips: 0 or 3.
    pub bom: usize,
    pub block: Option<Block>,
    /// gray-matter's `content`: the body after the closing `---` line, minus
    /// one `\r` and one `\n` right after the fence.
    pub content: &'a str,
}

/// Split `raw` the way gray-matter does:
///
/// - The note must start with `---` (after an optional BOM) and the fourth
///   character must not be `-`.
/// - The rest of the first line names the language when it is not blank.
/// - The block closes at the first `\n---` anywhere after that, even when
///   more text follows on that line; without one, the whole rest of the
///   note is frontmatter and the body is empty.
pub fn split(raw: &str) -> Split<'_> {
    let bom = if raw.starts_with('\u{feff}') { 3 } else { 0 };
    let s = &raw[bom..];
    let plain = Split {
        bom,
        block: None,
        content: s,
    };
    if !s.starts_with("---") || s[3..].starts_with('-') {
        return plain;
    }
    let mut start = bom + 3;
    let rest = &raw[start..];
    // `str.slice(0, str.search(/\r?\n/))`; without a newline, `slice(0, -1)`.
    let language_raw = match rest.find('\n') {
        Some(nl) => {
            &rest[..if nl > 0 && rest.as_bytes()[nl - 1] == b'\r' {
                nl - 1
            } else {
                nl
            }]
        }
        None => rest.char_indices().last().map_or("", |(i, _)| &rest[..i]),
    };
    let language = js::trim(language_raw).to_string();
    if !language.is_empty() {
        start += language_raw.len();
    }
    let close = raw[start..].find("\n---").map(|i| start + i);
    let matter_end = close.unwrap_or(raw.len());
    let content = match close {
        None => "",
        Some(c) => {
            let after = &raw[c + 4..];
            let after = after.strip_prefix('\r').unwrap_or(after);
            after.strip_prefix('\n').unwrap_or(after)
        }
    };
    Split {
        bom,
        block: Some(Block {
            language,
            matter: start..matter_end,
            close,
        }),
        content,
    }
}

/// A parsed note: its frontmatter data and its body.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed<'a> {
    pub data: Frontmatter,
    /// gray-matter's `content`, see [`Split::content`].
    pub content: &'a str,
    pub split: Split<'a>,
}

static COMMENT_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*#[^\n]+").expect("valid pattern"));

/// `matter(raw)`: the frontmatter as JSON, with the values js-yaml gives
/// (dates as ISO strings with `.000Z`), and the body.
///
/// A block of only comments and whitespace is empty frontmatter. A YAML
/// list becomes an object keyed by index and a scalar becomes an empty
/// object, as spreading gray-matter's data into an object did.
pub fn parse(raw: &str) -> Result<Parsed<'_>, FrontmatterError> {
    let split = split(raw);
    let Some(block) = &split.block else {
        return Ok(Parsed {
            data: Frontmatter::new(),
            content: split.content,
            split,
        });
    };
    let matter = &raw[block.matter.clone()];
    let value = if js::trim(&COMMENT_LINE.replace_all(matter, "")).is_empty() {
        Value::Null
    } else {
        parse_block(&block.language, matter)?
    };
    Ok(Parsed {
        data: to_frontmatter(value),
        content: split.content,
        split,
    })
}

fn parse_block(language: &str, matter: &str) -> Result<Value, FrontmatterError> {
    match language {
        "" | "yaml" | "yml" => load::load(matter)
            .map(|l| l.value)
            .map_err(FrontmatterError),
        "json" => serde_json::from_str(matter).map_err(|e| FrontmatterError(e.to_string())),
        other => Err(FrontmatterError(format!(
            "gray-matter engine \"{other}\" is not registered"
        ))),
    }
}

fn to_frontmatter(value: Value) -> Frontmatter {
    match value {
        Value::Object(map) => map,
        Value::Array(items) => items
            .into_iter()
            .enumerate()
            .map(|(i, v)| (i.to_string(), v))
            .collect(),
        _ => Frontmatter::new(),
    }
}

/// `matter.stringify(content, data)`: a fresh note from a body and its
/// frontmatter. Empty data writes no block. The body gets a final newline
/// when it has none.
pub fn stringify(content: &str, data: &Frontmatter) -> String {
    let mut out = String::new();
    if !data.is_empty() {
        let yaml = dump_yaml(&Value::Object(data.clone()));
        out.push_str("---\n");
        out.push_str(js::trim(&yaml));
        out.push_str("\n---\n");
    }
    out.push_str(content);
    if !content.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests;
