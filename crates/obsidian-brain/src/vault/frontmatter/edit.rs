//! Set and remove top-level frontmatter keys in place.

use serde_json::Value;

use super::load::{self, TopEntry, TopShape};
use super::{FrontmatterError, Parsed, dump, parse, stringify};
use crate::types::Frontmatter;
use crate::util::js;

/// One change to a note's frontmatter.
#[derive(Debug, Clone, PartialEq)]
pub enum FrontmatterOp {
    /// Set a top-level key. `Value::Null` writes `key: null`; callers that
    /// treat null as "remove" send [`FrontmatterOp::Remove`] instead.
    Set(String, Value),
    /// Remove a top-level key; a key that is not there is no change.
    Remove(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Edited {
    /// The whole note after the edit. Equal to the input when nothing changed.
    pub text: String,
    /// The frontmatter after the edit, in JS object key order.
    pub data: Frontmatter,
    /// False when the edit could not be made by line and the block was
    /// re-serialised instead.
    pub in_place: bool,
}

/// Apply `ops` in order to the frontmatter of `raw`.
///
/// Only the lines of keys whose value changes are touched: a changed key's
/// lines are replaced where they stand, a removed key's lines are dropped,
/// and a new key goes after the last key. Setting a key to the value it
/// has already is no change. A note without frontmatter gets a block; a
/// block left with no keys is dropped, body untouched. New lines use the
/// note's line ending.
///
/// Every result is parsed again and must give exactly the expected data
/// and the same body. When it does not, or the block cannot be edited by
/// line, the block is re-serialised with `matter.stringify` rules
/// (`in_place` is then false).
pub fn edit_frontmatter(raw: &str, ops: &[FrontmatterOp]) -> Result<Edited, FrontmatterError> {
    let parsed = parse(raw)?;
    let mut data = parsed.data.clone();
    for op in ops {
        match op {
            FrontmatterOp::Set(k, v) => {
                data.insert(k.clone(), v.clone());
            }
            FrontmatterOp::Remove(k) => {
                data.shift_remove(k);
            }
        }
    }
    js::js_object_order(&mut data);

    if data == parsed.data {
        return Ok(Edited {
            text: raw.to_string(),
            data,
            in_place: true,
        });
    }
    if let Some(text) = in_place(raw, &parsed, &data)
        && let Ok(check) = parse(&text)
        && check.data == data
        && check.content == parsed.content
    {
        return Ok(Edited {
            text,
            data,
            in_place: true,
        });
    }
    let text = stringify(parsed.content, &data);
    Ok(Edited {
        text,
        data,
        in_place: false,
    })
}

fn line_ending(s: &str) -> &'static str {
    match s.find('\n') {
        Some(i) if i > 0 && s.as_bytes()[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

/// js-yaml's text for one top-level entry, with `\n` line ends.
fn render_entry(key_text: Option<&str>, key: &str, value: &Value) -> String {
    let pair = dump::write_pair(0, key, value, false);
    match key_text {
        Some(text) => {
            let rendered = dump::dump_key(key);
            match pair.strip_prefix(&rendered) {
                Some(rest) => format!("{text}{rest}\n"),
                None => format!("{pair}\n"),
            }
        }
        None => format!("{pair}\n"),
    }
}

/// The source text of the key that starts `line` at byte `col`, up to the
/// `:` that ends it.
fn key_text(line: &str, col: usize) -> Option<&str> {
    let rest = line.get(col..)?;
    let bytes = rest.as_bytes();
    let end = match bytes.first()? {
        b'"' => {
            let mut i = 1;
            loop {
                match bytes.get(i)? {
                    b'\\' => i += 2,
                    b'"' => break i + 1,
                    _ => i += 1,
                }
            }
        }
        b'\'' => {
            let mut i = 1;
            loop {
                match bytes.get(i)? {
                    b'\'' if bytes.get(i + 1) == Some(&b'\'') => i += 2,
                    b'\'' => break i + 1,
                    _ => i += 1,
                }
            }
        }
        _ => {
            let mut i = 0;
            loop {
                if bytes.get(i)? == &b':'
                    && matches!(bytes.get(i + 1), None | Some(b' ' | b'\t' | b'\r' | b'\n'))
                {
                    break i;
                }
                i += 1;
            }
        }
    };
    let after = rest[end..].trim_start_matches([' ', '\t']);
    after.starts_with(':').then(|| js::trim_end(&rest[..end]))
}

/// The edited note, or `None` when the block cannot be edited by line.
fn in_place(raw: &str, parsed: &Parsed, data: &Frontmatter) -> Option<String> {
    let bom = &raw[..parsed.split.bom];
    let body = &raw[parsed.split.bom..];

    let Some(block) = &parsed.split.block else {
        // No block yet: write one in front of the untouched note.
        let eol = if body.contains("\r\n") { "\r\n" } else { "\n" };
        let yaml = dump::dump_yaml(&Value::Object(data.clone())).replace('\n', eol);
        return Some(format!("{bom}---{eol}{yaml}---{eol}{body}"));
    };

    if data.is_empty() {
        // The last key went: drop the block and keep the body as it is.
        return Some(format!("{bom}{}", parsed.content));
    }
    if !matches!(block.language.as_str(), "" | "yaml" | "yml") {
        return None;
    }
    let close = block.close?;
    let opening_end = raw[parsed.split.bom..].find('\n')? + parsed.split.bom + 1;
    if opening_end > close + 1 {
        return None;
    }
    let eol = line_ending(&raw[parsed.split.bom..]);

    // The YAML lines, each with its line ending. The matter text starts with
    // the rest of the opening line, so its line n is region line n - 1.
    let region = &raw[opening_end..=close];
    let lines: Vec<&str> = region.split_inclusive('\n').collect();
    let matter = &raw[block.matter.clone()];
    let entries: Vec<TopEntry> = match load::load(matter).ok()?.shape {
        TopShape::BlockMap(entries) => entries,
        TopShape::Empty => Vec::new(),
        TopShape::Other => return None,
    };
    let mut starts = Vec::with_capacity(entries.len());
    for e in &entries {
        let line = e.line.checked_sub(1)?;
        let text = lines.get(line)?;
        // The key must open its line, so `? key` and keys after another
        // node are refused.
        if text.chars().take(e.col).any(|c| c != ' ') {
            return None;
        }
        starts.push(line);
    }
    let base = entries.first().map_or(0, |e| e.col);

    // An entry runs from its key line to the next key, minus trailing blank
    // lines, comments that are not indented under it, and a `...` document
    // end.
    let trailing_inert = |l: &str| {
        let t = js::trim(l);
        let indent = l.len() - l.trim_start_matches(' ').len();
        t.is_empty() || (t.starts_with('#') && indent <= base) || (t == "..." && indent == 0)
    };
    let mut extents = Vec::with_capacity(entries.len());
    for (i, &start) in starts.iter().enumerate() {
        let next = starts.get(i + 1).copied().unwrap_or(lines.len());
        let mut end = next;
        while end > start + 1 && trailing_inert(lines[end - 1]) {
            end -= 1;
        }
        extents.push((start, end, next));
    }

    let render = |key_text: Option<&str>, key: &str, value: &Value| -> String {
        let pad = " ".repeat(base);
        let text = render_entry(key_text, key, value);
        text.split_inclusive('\n')
            .map(|l| format!("{pad}{l}"))
            .collect::<String>()
            .replace('\n', eol)
    };

    // Keys that are new, or that only came from a `<<` merge and changed.
    let mut added = String::new();
    for (k, v) in data {
        let has_entry = entries.iter().any(|e| !e.merge && &e.key == k);
        if !has_entry && parsed.data.get(k) != Some(v) {
            added.push_str(&render(None, k, v));
        }
    }

    let mut out = String::new();
    out.push_str(&raw[..opening_end]);
    let head_end = starts.first().copied().unwrap_or(lines.len());
    lines[..head_end].iter().for_each(|l| out.push_str(l));
    for (i, e) in entries.iter().enumerate() {
        let (start, end, next) = extents[i];
        let old = parsed.data.get(&e.key);
        match data.get(&e.key) {
            _ if e.merge => lines[start..end].iter().for_each(|l| out.push_str(l)),
            None => {}
            Some(new) if Some(new) != old => {
                let original_key = key_text(lines[start].trim_end_matches(['\r', '\n']), e.col);
                out.push_str(&render(original_key, &e.key, new));
            }
            Some(_) => lines[start..end].iter().for_each(|l| out.push_str(l)),
        }
        if i + 1 == entries.len() {
            // New keys go right after the last key's own lines.
            out.push_str(&added);
        }
        lines[end..next].iter().for_each(|l| out.push_str(l));
    }
    if entries.is_empty() {
        out.push_str(&added);
    }
    out.push_str(&raw[close + 1..]);
    Some(out)
}
