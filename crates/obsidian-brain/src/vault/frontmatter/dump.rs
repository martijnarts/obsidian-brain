//! A port of js-yaml 3's `safeDump` with its default options (indent 2,
//! line width 80), for JSON values.
//!
//! One deliberate change: js-yaml 3 escapes every character outside the
//! Basic Multilingual Plane (`"\U0001F389"`), because it tests UTF-16 code
//! units for printability. Here an emoji is printable, as in js-yaml 4, so
//! `title: 🎉 party` stays plain. Line widths count characters, not UTF-16
//! code units.

use serde_json::{Map, Value};

use super::load::resolves_implicitly;
use crate::util::js;

const INDENT: usize = 2;
const LINE_WIDTH: usize = 80;

const DEPRECATED_BOOLEANS_SYNTAX: [&str; 16] = [
    "y", "Y", "yes", "Yes", "YES", "on", "On", "ON", "n", "N", "no", "No", "NO", "off", "Off",
    "OFF",
];

/// `yaml.safeDump(value)`: the YAML text, ending in a newline.
pub fn dump_yaml(value: &Value) -> String {
    format!("{}\n", write_node(0, value, true, true, false))
}

/// The key as js-yaml writes it in a block mapping.
pub(super) fn dump_key(key: &str) -> String {
    write_scalar(key, 1, true)
}

fn next_line(level: usize) -> String {
    format!("\n{}", " ".repeat(INDENT * level))
}

fn write_node(level: usize, value: &Value, block: bool, compact: bool, iskey: bool) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => represent_number(n),
        Value::String(s) => write_scalar(s, level, iskey),
        Value::Array(items) => {
            if block && !items.is_empty() {
                write_block_sequence(level, items, compact)
            } else {
                write_flow_sequence(level, items)
            }
        }
        Value::Object(map) => {
            let ordered;
            let map = if map.keys().any(|k| k.parse::<u64>().is_ok()) {
                let mut m = map.clone();
                js::js_object_order(&mut m);
                ordered = m;
                &ordered
            } else {
                map
            };
            if block && !map.is_empty() {
                write_block_mapping(level, map, compact)
            } else {
                write_flow_mapping(level, map)
            }
        }
    }
}

fn represent_number(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64().filter(|i| i.unsigned_abs() <= 1 << 53) {
        return i.to_string();
    }
    let x = js::as_f64(n);
    if x.fract() == 0.0 && !(x == 0.0 && x.is_sign_negative()) {
        return js::number_to_string(x);
    }
    if x == 0.0 {
        return "-0.0".into();
    }
    let res = js::number_to_string(x);
    // JS writes `5e-100`; YAML needs `5.e-100`.
    let digits_then_e = res.trim_start_matches(['-', '+']);
    let int_part = digits_then_e.split('e').next().unwrap_or("");
    if res.contains('e') && !int_part.is_empty() && int_part.bytes().all(|b| b.is_ascii_digit()) {
        res.replacen('e', ".e", 1)
    } else {
        res
    }
}

fn write_flow_sequence(level: usize, items: &[Value]) -> String {
    let parts: Vec<String> = items
        .iter()
        .map(|v| write_node(level, v, false, false, false))
        .collect();
    format!("[{}]", parts.join(", "))
}

fn write_block_sequence(level: usize, items: &[Value], compact: bool) -> String {
    let mut out = String::new();
    for (i, item) in items.iter().enumerate() {
        let dump = write_node(level + 1, item, true, true, false);
        if !compact || i != 0 {
            out.push_str(&next_line(level));
        }
        out.push_str(if dump.starts_with('\n') { "-" } else { "- " });
        out.push_str(&dump);
    }
    out
}

fn write_flow_mapping(level: usize, map: &Map<String, Value>) -> String {
    let mut out = String::new();
    for (i, (k, v)) in map.iter().enumerate() {
        if i != 0 {
            out.push_str(", ");
        }
        let key = write_node(level, &Value::String(k.clone()), false, false, false);
        if key.chars().count() > 1024 {
            out.push_str("? ");
        }
        out.push_str(&key);
        out.push_str(": ");
        out.push_str(&write_node(level, v, false, false, false));
    }
    format!("{{{out}}}")
}

fn write_block_mapping(level: usize, map: &Map<String, Value>, compact: bool) -> String {
    let mut out = String::new();
    for (i, (k, v)) in map.iter().enumerate() {
        out.push_str(&write_pair(level, k, v, !compact || i != 0));
    }
    out
}

/// One `key: value` pair of a block mapping at `level`.
pub(super) fn write_pair(level: usize, key: &str, value: &Value, newline_first: bool) -> String {
    let mut pair = String::new();
    if newline_first {
        pair.push_str(&next_line(level));
    }
    let key_dump = write_scalar(key, level + 1, true);
    let explicit = key_dump.chars().count() > 1024;
    if explicit {
        pair.push_str(if key_dump.starts_with('\n') {
            "?"
        } else {
            "? "
        });
    }
    pair.push_str(&key_dump);
    if explicit {
        pair.push_str(&next_line(level));
    }
    let value_dump = write_node(level + 1, value, true, explicit, false);
    pair.push_str(if value_dump.starts_with('\n') {
        ":"
    } else {
        ": "
    });
    pair.push_str(&value_dump);
    pair
}

#[derive(Debug, PartialEq, Eq)]
enum Style {
    Plain,
    Single,
    Literal,
    Folded,
    Double,
}

fn is_whitespace(c: char) -> bool {
    c == ' ' || c == '\t'
}

fn is_printable(c: char) -> bool {
    let c = c as u32;
    (0x20..=0x7E).contains(&c)
        || ((0xA1..=0xD7FF).contains(&c) && c != 0x2028 && c != 0x2029)
        || ((0xE000..=0xFFFD).contains(&c) && c != 0xFEFF)
        || (0x10000..=0x10FFFF).contains(&c)
}

fn is_ns_char(c: char) -> bool {
    is_printable(c) && !is_whitespace(c) && c != '\u{feff}' && c != '\r' && c != '\n'
}

fn is_plain_safe(c: char, prev: Option<char>) -> bool {
    is_printable(c)
        && c != '\u{feff}'
        && !",[]{}".contains(c)
        && c != ':'
        && (c != '#' || prev.is_some_and(|p| p != '\0' && is_ns_char(p)))
}

fn is_plain_safe_first(c: char) -> bool {
    is_printable(c) && c != '\u{feff}' && !is_whitespace(c) && !"-?:,[]{}#&*!|=>'\"%@`".contains(c)
}

fn need_indent_indicator(s: &[char]) -> bool {
    s.iter().find(|c| **c != '\n') == Some(&' ')
}

fn choose_scalar_style(s: &[char], single_line_only: bool, line_width: usize) -> Style {
    let mut has_line_break = false;
    let mut has_foldable_line = false;
    let mut previous_line_break: isize = -1;
    let mut plain = is_plain_safe_first(s[0]) && !is_whitespace(s[s.len() - 1]);
    let foldable = |i: usize, prev_lb: isize| -> bool {
        (i as isize - prev_lb - 1) as usize > line_width
            && s.get((prev_lb + 1) as usize) != Some(&' ')
    };

    for (i, &c) in s.iter().enumerate() {
        if !single_line_only && c == '\n' {
            has_line_break = true;
            has_foldable_line = has_foldable_line || foldable(i, previous_line_break);
            previous_line_break = i as isize;
        } else if !is_printable(c) {
            return Style::Double;
        }
        let prev = if i > 0 { Some(s[i - 1]) } else { None };
        plain = plain && is_plain_safe(c, prev);
    }
    if !single_line_only {
        has_foldable_line = has_foldable_line || foldable(s.len(), previous_line_break);
    }
    if !has_line_break && !has_foldable_line {
        let text: String = s.iter().collect();
        return if plain && !resolves_implicitly(&text) {
            Style::Plain
        } else {
            Style::Single
        };
    }
    if has_foldable_line {
        Style::Folded
    } else {
        Style::Literal
    }
}

fn write_scalar(string: &str, level: usize, iskey: bool) -> String {
    if string.is_empty() {
        return "''".into();
    }
    if DEPRECATED_BOOLEANS_SYNTAX.contains(&string) {
        return format!("'{string}'");
    }
    let indent = INDENT * level.max(1);
    let line_width = LINE_WIDTH.min(40).max(LINE_WIDTH.saturating_sub(indent));
    let chars: Vec<char> = string.chars().collect();
    match choose_scalar_style(&chars, iskey, line_width) {
        Style::Plain => string.to_string(),
        Style::Single => format!("'{}'", string.replace('\'', "''")),
        Style::Literal => format!(
            "|{}{}",
            block_header(&chars),
            drop_ending_newline(&indent_string(string, indent))
        ),
        Style::Folded => {
            let folded = fold_string(&chars, line_width);
            format!(
                ">{}{}",
                block_header(&chars),
                drop_ending_newline(&indent_string(&folded, indent))
            )
        }
        Style::Double => format!("\"{}\"", escape_string(string)),
    }
}

/// Indent every line; empty lines (`\n` only) are not indented.
fn indent_string(s: &str, spaces: usize) -> String {
    let ind = " ".repeat(spaces);
    let mut out = String::new();
    for line in s.split_inclusive('\n') {
        if !line.is_empty() && line != "\n" {
            out.push_str(&ind);
        }
        out.push_str(line);
    }
    out
}

fn block_header(s: &[char]) -> String {
    let indicator = if need_indent_indicator(s) {
        INDENT.to_string()
    } else {
        String::new()
    };
    // The string "\n" counts as a trailing empty line.
    let clip = s.last() == Some(&'\n');
    let keep = clip && (s.len() >= 2 && s[s.len() - 2] == '\n' || s.len() == 1);
    let chomp = if keep {
        "+"
    } else if clip {
        ""
    } else {
        "-"
    };
    format!("{indicator}{chomp}\n")
}

fn drop_ending_newline(s: &str) -> &str {
    s.strip_suffix('\n').unwrap_or(s)
}

/// In folded style, k consecutive newlines output as k+1 newlines, unless
/// they are before or after a more-indented line, or at the very beginning
/// or end, in which case k maps to k.
fn fold_string(s: &[char], width: usize) -> String {
    let first_lf = s.iter().position(|c| *c == '\n').unwrap_or(s.len());
    let mut result = fold_line(&s[..first_lf], width);
    let mut prev_more_indented = s.first() == Some(&'\n') || s.first() == Some(&' ');
    let mut i = first_lf;
    while i < s.len() {
        // `(\n+)([^\n]*)`
        let prefix_start = i;
        while i < s.len() && s[i] == '\n' {
            i += 1;
        }
        let prefix: String = s[prefix_start..i].iter().collect();
        let line_start = i;
        while i < s.len() && s[i] != '\n' {
            i += 1;
        }
        let line = &s[line_start..i];
        let more_indented = line.first() == Some(&' ');
        result.push_str(&prefix);
        if !prev_more_indented && !more_indented && !line.is_empty() {
            result.push('\n');
        }
        result.push_str(&fold_line(line, width));
        prev_more_indented = more_indented;
    }
    result
}

/// Greedy line breaking: the longest line under the limit each time, or
/// else the shortest line over it. A more-indented line is never folded.
fn fold_line(line: &[char], width: usize) -> String {
    if line.is_empty() || line[0] == ' ' {
        return line.iter().collect();
    }
    let slice = |a: usize, b: usize| line[a..b].iter().collect::<String>();
    let (mut start, mut curr) = (0usize, 0usize);
    let mut result = String::new();
    // Matches of `/ [^ ]/g`: each match consumes two characters.
    let mut i = 0;
    while i + 1 < line.len() {
        if line[i] == ' ' && line[i + 1] != ' ' {
            let next = i;
            if next - start > width {
                let end = if curr > start { curr } else { next };
                result.push('\n');
                result.push_str(&slice(start, end));
                start = end + 1;
            }
            curr = next;
            i += 2;
        } else {
            i += 1;
        }
    }
    result.push('\n');
    if line.len() - start > width && curr > start {
        result.push_str(&slice(start, curr));
        result.push('\n');
        result.push_str(&slice(curr + 1, line.len()));
    } else {
        result.push_str(&slice(start, line.len()));
    }
    result[1..].to_string()
}

fn escape_string(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let seq = match c {
            '\0' => Some("\\0"),
            '\u{7}' => Some("\\a"),
            '\u{8}' => Some("\\b"),
            '\t' => Some("\\t"),
            '\n' => Some("\\n"),
            '\u{b}' => Some("\\v"),
            '\u{c}' => Some("\\f"),
            '\r' => Some("\\r"),
            '\u{1b}' => Some("\\e"),
            '"' => Some("\\\""),
            '\\' => Some("\\\\"),
            '\u{85}' => Some("\\N"),
            '\u{a0}' => Some("\\_"),
            '\u{2028}' => Some("\\L"),
            '\u{2029}' => Some("\\P"),
            _ => None,
        };
        match seq {
            Some(seq) => out.push_str(seq),
            None if is_printable(c) => out.push(c),
            None => {
                let cp = c as u32;
                out.push_str(&if cp <= 0xFF {
                    format!("\\x{cp:02X}")
                } else if cp <= 0xFFFF {
                    format!("\\u{cp:04X}")
                } else {
                    format!("\\U{cp:08X}")
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keeps_emoji_plain() {
        assert_eq!(
            dump_yaml(&json!({"emoji": "🎉 party"})),
            "emoji: 🎉 party\n"
        );
    }

    #[test]
    fn dumps_collections_like_js_yaml() {
        let v = json!({"tags": ["a", "b"], "meta": {"source": "x", "stats": {"count": 3}}, "e": [], "o": {}, "list": [{"a": 1, "b": 2}, [1, 2]]});
        let want = "tags:\n  - a\n  - b\nmeta:\n  source: x\n  stats:\n    count: 3\ne: []\no: {}\nlist:\n  - a: 1\n    b: 2\n  - - 1\n    - 2\n";
        assert_eq!(dump_yaml(&v), want);
    }

    #[test]
    fn folds_long_lines() {
        let long = "word ".repeat(30);
        let out = dump_yaml(&json!({ "k": long.trim_end() }));
        assert!(out.starts_with("k: >-\n  word word"), "{out}");
        assert!(out.lines().all(|l| l.chars().count() <= 80));
    }

    #[test]
    fn numbers() {
        assert_eq!(dump_yaml(&json!(1e21)), "1e+21\n");
        assert_eq!(dump_yaml(&json!(1e-7)), "1.e-7\n");
        assert_eq!(dump_yaml(&json!(-0.0)), "-0.0\n");
        assert_eq!(dump_yaml(&json!(2.0)), "2\n");
    }
}
