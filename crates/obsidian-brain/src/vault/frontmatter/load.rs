//! YAML to JSON with the results js-yaml 3's `safeLoad` gives, on the
//! `yaml-rust2` event parser.
//!
//! js-yaml resolves plain scalars by YAML 1.1-flavoured rules that differ
//! from YAML 1.2: `0123` is octal, `1:30` is sexagesimal, `2024-01-01` is a
//! date (here an ISO string with `.000Z`), and `yes` is a string. Mapping
//! keys become JS strings, a duplicate key is an error, `<<` merges, and
//! keys that look like array indices sort first, as in a JS object.
//!
//! Where the two parsers disagree on syntax, `yaml-rust2` decides, with one
//! exception: js-yaml does not count tabs as indentation, so a block that
//! `yaml-rust2` refuses for a tab in its indentation is parsed again with
//! those tabs dropped.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};
use yaml_rust2::parser::{Event, Parser, Tag};
use yaml_rust2::scanner::{Marker, TScalarStyle};

use crate::util::js;

/// A top-level key of a block mapping, where it starts in the YAML text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TopEntry {
    pub key: String,
    /// 0-based line of the key in the YAML text.
    pub line: usize,
    /// 0-based column of the key, in characters.
    pub col: usize,
    /// A `<<` merge entry, which adds keys without being one.
    pub merge: bool,
}

/// How the top level of a document is laid out, for in-place edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TopShape {
    /// A block mapping whose keys start lines.
    BlockMap(Vec<TopEntry>),
    /// No document, or a null one: only comments and blank lines.
    Empty,
    /// Anything an edit cannot address by line: a flow mapping, a sequence,
    /// a scalar, a complex key, or tab indentation.
    Other,
}

#[derive(Debug)]
pub(super) struct Loaded {
    pub value: Value,
    pub shape: TopShape,
}

/// Parse one YAML document. The error is the parser's message.
pub(super) fn load(yaml: &str) -> Result<Loaded, String> {
    match load_once(yaml) {
        Ok(loaded) => Ok(loaded),
        Err(err) if err.contains("tabs disallowed") => {
            let untabbed = drop_indentation_tabs(yaml);
            if untabbed == yaml {
                return Err(err);
            }
            let mut loaded = load_once(&untabbed)?;
            loaded.shape = TopShape::Other;
            Ok(loaded)
        }
        Err(err) => Err(err),
    }
}

/// js-yaml counts leading spaces up to the first tab as indentation and
/// skips the rest of the leading whitespace.
fn drop_indentation_tabs(yaml: &str) -> String {
    yaml.split_inclusive('\n')
        .map(|line| {
            let spaces = line.len() - line.trim_start_matches(' ').len();
            if line[spaces..].starts_with('\t') {
                let rest = line[spaces..].trim_start_matches([' ', '\t']);
                format!("{}{rest}", " ".repeat(spaces))
            } else {
                line.to_string()
            }
        })
        .collect()
}

fn load_once(yaml: &str) -> Result<Loaded, String> {
    let mut b = Builder { parser: Parser::new_from_str(yaml), anchors: HashMap::new(), text: yaml };
    let mut value = Value::Null;
    let mut shape = TopShape::Empty;
    let mut documents = 0;
    loop {
        let (ev, mark) = b.next()?;
        match ev {
            Event::StreamStart | Event::DocumentEnd | Event::Nothing => {}
            Event::StreamEnd => break,
            Event::DocumentStart => {
                documents += 1;
                if documents > 1 {
                    return Err("expected a single document in the stream, but found more".into());
                }
            }
            ev => {
                let mut entries = Some(Vec::new());
                let flow = matches!(ev, Event::MappingStart(..)) && b.char_at(&mark) == Some('{');
                let is_map = matches!(ev, Event::MappingStart(..));
                value = b.node(ev, mark, if is_map && !flow { entries.as_mut() } else { None })?;
                shape = match (&value, is_map, flow, entries) {
                    (Value::Null, ..) => TopShape::Empty,
                    (_, true, false, Some(e)) => TopShape::BlockMap(e),
                    _ => TopShape::Other,
                };
            }
        }
    }
    Ok(Loaded { value, shape })
}

struct Builder<'a> {
    parser: Parser<std::str::Chars<'a>>,
    anchors: HashMap<usize, Value>,
    text: &'a str,
}

impl Builder<'_> {
    fn next(&mut self) -> Result<(Event, Marker), String> {
        self.parser.next_token().map_err(|e| e.to_string())
    }

    fn char_at(&self, mark: &Marker) -> Option<char> {
        self.text.chars().nth(mark.index())
    }

    /// Build the node that starts with `ev`. With `entries`, record the keys
    /// of this mapping as top-level entries.
    fn node(&mut self, ev: Event, mark: Marker, entries: Option<&mut Vec<TopEntry>>) -> Result<Value, String> {
        match ev {
            Event::Scalar(text, style, anchor, tag) => {
                let v = resolve_scalar(&text, style, tag.as_ref(), &mark)?;
                self.remember(anchor, &v);
                Ok(v)
            }
            Event::Alias(id) => self.anchors.get(&id).cloned().ok_or_else(|| "unidentified alias".to_string()),
            Event::SequenceStart(anchor, tag) => {
                let mut items = Vec::new();
                loop {
                    let (ev, mark) = self.next()?;
                    if ev == Event::SequenceEnd {
                        break;
                    }
                    items.push(self.node(ev, mark, None)?);
                }
                let v = construct_sequence(items, tag.as_ref())?;
                self.remember(anchor, &v);
                Ok(v)
            }
            Event::MappingStart(anchor, tag) => {
                let v = self.mapping(entries)?;
                let v = construct_mapping(v, tag.as_ref())?;
                self.remember(anchor, &v);
                Ok(v)
            }
            other => Err(format!("unexpected YAML event {other:?}")),
        }
    }

    fn mapping(&mut self, mut entries: Option<&mut Vec<TopEntry>>) -> Result<Map<String, Value>, String> {
        let mut out = Map::new();
        let mut overridable: HashSet<String> = HashSet::new();
        loop {
            let (ev, key_mark) = self.next()?;
            if ev == Event::MappingEnd {
                break;
            }
            let is_merge = matches!(&ev, Event::Scalar(t, TScalarStyle::Plain, _, None) if t == "<<")
                || matches!(&ev, Event::Scalar(_, _, _, Some(t)) if is_core(t, "merge"));
            let complex = !matches!(ev, Event::Scalar(..) | Event::Alias(_));
            let key_node = self.node(ev, key_mark, None)?;
            let (value_ev, value_mark) = self.next()?;
            let value = self.node(value_ev, value_mark, None)?;

            if let Some(e) = entries.as_deref_mut() {
                if complex {
                    // A complex key cannot be edited by line.
                    entries = None;
                } else {
                    e.push(TopEntry {
                        key: key_string(&key_node)?,
                        line: key_mark.line().saturating_sub(1),
                        col: key_mark.col(),
                        merge: is_merge,
                    });
                }
            }

            if is_merge {
                merge_into(&mut out, &mut overridable, value)?;
                continue;
            }
            let key = key_string(&key_node)?;
            if out.contains_key(&key) && !overridable.contains(&key) {
                return Err(format!("duplicated mapping key at line {}, column {}", key_mark.line(), key_mark.col() + 1));
            }
            overridable.remove(&key);
            out.insert(key, value);
        }
        js::js_object_order(&mut out);
        Ok(out)
    }

    fn remember(&mut self, anchor: usize, v: &Value) {
        if anchor > 0 {
            self.anchors.insert(anchor, v.clone());
        }
    }
}

fn merge_into(out: &mut Map<String, Value>, overridable: &mut HashSet<String>, source: Value) -> Result<(), String> {
    let sources = match source {
        Value::Object(m) => vec![m],
        Value::Array(items) => items
            .into_iter()
            .map(|item| match item {
                Value::Object(m) => Ok(m),
                _ => Err("cannot merge mappings; the provided source object is unacceptable".to_string()),
            })
            .collect::<Result<_, _>>()?,
        _ => return Err("cannot merge mappings; the provided source object is unacceptable".into()),
    };
    for m in sources {
        for (k, v) in m {
            if !out.contains_key(&k) {
                overridable.insert(k.clone());
                out.insert(k, v);
            }
        }
    }
    Ok(())
}

/// `String(key)` as js-yaml stores a mapping key.
fn key_string(key: &Value) -> Result<String, String> {
    Ok(match key {
        Value::Array(items) => {
            if items.iter().any(Value::is_array) {
                return Err("nested arrays are not supported inside keys".into());
            }
            js::to_js_string(key)
        }
        other => js::to_js_string(other),
    })
}

fn is_core(tag: &Tag, name: &str) -> bool {
    tag.handle == "tag:yaml.org,2002:" && tag.suffix == name
}

fn tag_name(tag: &Tag) -> String {
    if tag.handle == "!" { format!("!{}", tag.suffix) } else { format!("{}{}", tag.handle, tag.suffix) }
}

fn resolve_scalar(text: &str, style: TScalarStyle, tag: Option<&Tag>, mark: &Marker) -> Result<Value, String> {
    let Some(tag) = tag else {
        return Ok(if style == TScalarStyle::Plain { resolve_plain(text) } else { Value::String(text.to_string()) });
    };
    let explicit = |kind: &str, v: Option<Value>| {
        v.ok_or_else(|| {
            format!("cannot resolve a node with !<tag:yaml.org,2002:{kind}> explicit tag at line {}, column {}", mark.line(), mark.col() + 1)
        })
    };
    if tag.handle == "!" && tag.suffix.is_empty() {
        return Ok(Value::String(text.to_string()));
    }
    if tag.handle != "tag:yaml.org,2002:" {
        return Err(format!("unknown tag !<{}> at line {}, column {}", tag_name(tag), mark.line(), mark.col() + 1));
    }
    match tag.suffix.as_str() {
        "str" => Ok(Value::String(text.to_string())),
        "null" => explicit("null", is_null(text).then_some(Value::Null)),
        "bool" => explicit("bool", resolve_bool(text).map(Value::Bool)),
        "int" => explicit("int", resolve_int(text).map(js::number_value)),
        "float" => explicit("float", resolve_float(text).map(js::number_value)),
        "timestamp" => explicit("timestamp", resolve_timestamp(text).map(Value::String)),
        "merge" => Ok(Value::String(text.to_string())),
        // js-yaml gives a Buffer; its base64 text is the closest JSON form.
        "binary" => Ok(Value::String(text.to_string())),
        other => Err(format!("unknown tag !<tag:yaml.org,2002:{other}> at line {}, column {}", mark.line(), mark.col() + 1)),
    }
}

fn construct_sequence(items: Vec<Value>, tag: Option<&Tag>) -> Result<Value, String> {
    let Some(tag) = tag else { return Ok(Value::Array(items)) };
    match (tag.handle.as_str(), tag.suffix.as_str()) {
        ("tag:yaml.org,2002:", "seq" | "omap") | ("!", "") => Ok(Value::Array(items)),
        ("tag:yaml.org,2002:", "pairs") => items
            .into_iter()
            .map(|item| match item {
                Value::Object(m) if m.len() == 1 => {
                    let (k, v) = m.into_iter().next().expect("one entry");
                    Ok(Value::Array(vec![Value::String(k), v]))
                }
                _ => Err("cannot resolve a node with !<tag:yaml.org,2002:pairs> explicit tag".to_string()),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        _ => Err(format!("unknown tag !<{}>", tag_name(tag))),
    }
}

fn construct_mapping(map: Map<String, Value>, tag: Option<&Tag>) -> Result<Value, String> {
    match tag {
        None => Ok(Value::Object(map)),
        Some(t) if (t.handle == "tag:yaml.org,2002:" && (t.suffix == "map" || t.suffix == "set")) || (t.handle == "!" && t.suffix.is_empty()) => {
            Ok(Value::Object(map))
        }
        Some(t) => Err(format!("unknown tag !<{}>", tag_name(t))),
    }
}

fn is_null(s: &str) -> bool {
    matches!(s, "~" | "null" | "Null" | "NULL" | "")
}

fn resolve_bool(s: &str) -> Option<bool> {
    match s {
        "true" | "True" | "TRUE" => Some(true),
        "false" | "False" | "FALSE" => Some(false),
        _ => None,
    }
}

/// js-yaml's `resolveYamlInteger` + `constructYamlInteger`.
fn resolve_int(data: &str) -> Option<f64> {
    let b = data.as_bytes();
    let max = b.len();
    if max == 0 {
        return None;
    }
    let mut index = 0;
    let mut ch = b[0];
    if ch == b'-' || ch == b'+' {
        index += 1;
        ch = *b.get(index)?;
    }
    let digits_ok = |from: usize, ok: fn(u8) -> bool| -> bool {
        let mut has = false;
        let mut last = 0u8;
        for &c in &b[from..] {
            last = c;
            if c == b'_' {
                continue;
            }
            if !ok(c) {
                return false;
            }
            has = true;
        }
        has && last != b'_'
    };
    let valid = if ch == b'0' {
        if index + 1 == max {
            true
        } else {
            match b[index + 1] {
                b'b' => digits_ok(index + 2, |c| c == b'0' || c == b'1'),
                b'x' => digits_ok(index + 2, |c| c.is_ascii_hexdigit()),
                _ => digits_ok(index + 1, |c| (b'0'..=b'7').contains(&c)),
            }
        }
    } else {
        if ch == b'_' {
            return None;
        }
        let mut has = false;
        let mut i = index;
        let mut last = 0u8;
        while i < max {
            last = b[i];
            if last == b'_' {
                i += 1;
                continue;
            }
            if last == b':' {
                break;
            }
            if !last.is_ascii_digit() {
                return None;
            }
            has = true;
            i += 1;
        }
        if !has || last == b'_' {
            return None;
        }
        if last != b':' { true } else { BASE60_TAIL.is_match(&data[i..]) }
    };
    if !valid {
        return None;
    }

    let mut value: String = data.chars().filter(|c| *c != '_').collect();
    let mut sign = 1.0;
    if value.starts_with(['-', '+']) {
        if value.starts_with('-') {
            sign = -1.0;
        }
        value.remove(0);
    }
    if value == "0" {
        return Some(0.0);
    }
    let radix = |digits: &str, r: u32| -> f64 { digits.chars().fold(0.0, |acc, c| acc * r as f64 + c.to_digit(r).unwrap_or(0) as f64) };
    if let Some(rest) = value.strip_prefix('0') {
        if let Some(bin) = rest.strip_prefix('b') {
            return Some(sign * radix(bin, 2));
        }
        if let Some(hex) = rest.strip_prefix('x') {
            return Some(sign * radix(hex, 16));
        }
        return Some(sign * radix(rest, 8));
    }
    if value.contains(':') {
        let mut total = 0.0;
        let mut base = 1.0;
        for part in value.split(':').rev() {
            total += part.parse::<f64>().unwrap_or(0.0) * base;
            base *= 60.0;
        }
        return Some(sign * total);
    }
    Some(sign * value.parse::<f64>().ok()?)
}

static BASE60_TAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(:[0-5]?[0-9])+$").expect("valid pattern"));
static FLOAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^(?:[-+]?(?:0|[1-9][0-9_]*)(?:\.[0-9_]*)?(?:[eE][-+]?[0-9]+)?",
        r"|\.[0-9_]+(?:[eE][-+]?[0-9]+)?",
        r"|[-+]?[0-9][0-9_]*(?::[0-5]?[0-9])+\.[0-9_]*",
        r"|[-+]?\.(?:inf|Inf|INF)",
        r"|\.(?:nan|NaN|NAN))$"
    ))
    .expect("valid pattern")
});
static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([0-9]{4})-([0-9]{2})-([0-9]{2})$").expect("valid pattern"));
static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^([0-9]{4})-([0-9][0-9]?)-([0-9][0-9]?)(?:[Tt]|[ \t]+)([0-9][0-9]?):([0-9][0-9]):([0-9][0-9])",
        r"(?:\.([0-9]*))?(?:[ \t]*(Z|([-+])([0-9][0-9]?)(?::([0-9][0-9]))?))?$"
    ))
    .expect("valid pattern")
});

/// js-yaml's `resolveYamlFloat` + `constructYamlFloat`.
fn resolve_float(data: &str) -> Option<f64> {
    if !FLOAT.is_match(data) || data.ends_with('_') {
        return None;
    }
    let mut value = data.replace('_', "").to_lowercase();
    let sign = if value.starts_with('-') { -1.0 } else { 1.0 };
    if value.starts_with(['-', '+']) {
        value.remove(0);
    }
    if value == ".inf" {
        return Some(sign * f64::INFINITY);
    }
    if value == ".nan" {
        return Some(f64::NAN);
    }
    if value.contains(':') {
        let mut total = 0.0;
        let mut base = 1.0;
        for part in value.split(':').rev() {
            total += parse_float_prefix(part) * base;
            base *= 60.0;
        }
        return Some(sign * total);
    }
    Some(sign * parse_float_prefix(&value))
}

/// `parseFloat`: the longest prefix that is a number.
fn parse_float_prefix(s: &str) -> f64 {
    (1..=s.len()).rev().filter(|&n| s.is_char_boundary(n)).find_map(|n| s[..n].parse::<f64>().ok()).unwrap_or(f64::NAN)
}

/// Days from 1970-01-01 to the given proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// `Date.UTC(year, month, day, h, mi, s, ms)` in milliseconds, with the
/// same overflow rules: month 12 is January of the next year, day 0 is the
/// last day of the previous month, and years 0 to 99 mean 1900 to 1999.
fn date_utc(year: i64, month: i64, day: i64, h: i64, mi: i64, s: i64, ms: i64) -> i64 {
    let year = if (0..=99).contains(&year) { 1900 + year } else { year };
    let y = year + month.div_euclid(12);
    let m = month.rem_euclid(12) + 1;
    let days = days_from_civil(y, m, 1) + day - 1;
    days * 86_400_000 + h * 3_600_000 + mi * 60_000 + s * 1000 + ms
}

/// `Date.prototype.toISOString`.
fn iso_string(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    // civil_from_days
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    let year = if (0..=9999).contains(&y) {
        format!("{y:04}")
    } else {
        format!("{}{:06}", if y < 0 { '-' } else { '+' }, y.abs())
    };
    format!(
        "{year}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000
    )
}

/// js-yaml's timestamp type, as the ISO string `JSON.stringify` makes of
/// the `Date`.
fn resolve_timestamp(data: &str) -> Option<String> {
    let num = |s: Option<regex::Match>| s.map_or(0, |m| m.as_str().parse::<i64>().unwrap_or(0));
    if let Some(m) = DATE.captures(data) {
        return Some(iso_string(date_utc(num(m.get(1)), num(m.get(2)) - 1, num(m.get(3)), 0, 0, 0, 0)));
    }
    let m = TIMESTAMP.captures(data)?;
    let fraction = m.get(7).map_or(0, |f| {
        let mut digits: String = f.as_str().chars().take(3).collect();
        while digits.len() < 3 {
            digits.push('0');
        }
        digits.parse::<i64>().unwrap_or(0)
    });
    let mut ms = date_utc(num(m.get(1)), num(m.get(2)) - 1, num(m.get(3)), num(m.get(4)), num(m.get(5)), num(m.get(6)), fraction);
    if let Some(sign) = m.get(9) {
        let delta = (num(m.get(10)) * 60 + num(m.get(11))) * 60_000;
        ms -= if sign.as_str() == "-" { -delta } else { delta };
    }
    Some(iso_string(ms))
}

/// A plain scalar with js-yaml's implicit types: null, bool, int, float,
/// timestamp.
fn resolve_plain(text: &str) -> Value {
    if is_null(text) {
        return Value::Null;
    }
    if let Some(b) = resolve_bool(text) {
        return Value::Bool(b);
    }
    if let Some(n) = resolve_int(text) {
        return js::number_value(n);
    }
    if let Some(n) = resolve_float(text) {
        return js::number_value(n);
    }
    if let Some(iso) = resolve_timestamp(text) {
        return Value::String(iso);
    }
    Value::String(text.to_string())
}

/// True when a plain scalar `s` would not read back as the string `s`: the
/// dumper quotes such strings.
pub(super) fn resolves_implicitly(s: &str) -> bool {
    s == "<<"
        || is_null(s)
        || resolve_bool(s).is_some()
        || resolve_int(s).is_some()
        || resolve_float(s).is_some()
        || DATE.is_match(s)
        || TIMESTAMP.is_match(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn val(yaml: &str) -> Value {
        load(yaml).unwrap().value
    }

    #[test]
    fn resolves_scalars_like_js_yaml() {
        let v = val(
            "d: 2024-01-01\nt: 2024-01-01 10:00:00 +02:00\nbad: 2024-13-45\nold: 0024-01-01\noct: 0123\nzip: 09\nsex: 1:30\nf: 1.0\ne: 1e3\nn: .nan\ni: -.inf\nh: 0x1F\nb: 0b101\nneg0: -0\nbig: 12345678901234567890\nyes: yes\nT: True\nnul: Null\nund: 1_000\nfrac: 2024-01-01T10:00:00.5Z\nq: '12'\n",
        );
        assert_eq!(
            v,
            json!({"d":"2024-01-01T00:00:00.000Z","t":"2024-01-01T08:00:00.000Z","bad":"2025-02-14T00:00:00.000Z","old":"1924-01-01T00:00:00.000Z","oct":83,"zip":"09","sex":90,"f":1,"e":1000,"n":null,"i":null,"h":31,"b":5,"neg0":0,"big":12345678901234567000u64,"yes":"yes","T":true,"nul":null,"und":1000,"frac":"2024-01-01T10:00:00.500Z","q":"12"})
        );
    }

    #[test]
    fn keys_order_and_merges() {
        let v = val("b: 1\n2: x\na: 3\n1: y\n");
        assert_eq!(v.as_object().unwrap().keys().collect::<Vec<_>>(), ["1", "2", "b", "a"]);
        assert_eq!(val("'1': a\ntrue: b\n~: c\n[x, y]: d\n"), json!({"1": "a", "true": "b", "null": "c", "x,y": "d"}));
        assert_eq!(
            val("base: &b {x: 1}\nother:\n  <<: *b\n  y: 2\n  x: 3\n"),
            json!({"base": {"x": 1}, "other": {"x": 3, "y": 2}})
        );
    }

    #[test]
    fn errors_like_js_yaml() {
        assert!(load("a: 1\na: 2\n").unwrap_err().contains("duplicated mapping key"));
        assert!(load("a: !foo x\n").unwrap_err().contains("unknown tag"));
        assert_eq!(val("a: !!str 12\nb: !!int '7'\n"), json!({"a": "12", "b": 7}));
    }

    #[test]
    fn tab_indentation_reads_as_js_yaml_does() {
        let l = load("title: Test\nnested:\n\tvalue: ok\n").unwrap();
        assert_eq!(l.value, json!({"title": "Test", "nested": null, "value": "ok"}));
        assert_eq!(l.shape, TopShape::Other);
    }

    #[test]
    fn records_top_level_entries() {
        let l = load("\n# c\na: 1\n\"q k\": |\n  x\nl:\n- 1\n").unwrap();
        let TopShape::BlockMap(e) = l.shape else { panic!("{:?}", l.shape) };
        let keys: Vec<_> = e.iter().map(|e| (e.key.as_str(), e.line, e.col)).collect();
        assert_eq!(keys, [("a", 2, 0), ("q k", 3, 0), ("l", 5, 0)]);
        assert_eq!(load("{a: 1}").unwrap().shape, TopShape::Other);
        assert_eq!(load("# only\n").unwrap().shape, TopShape::Empty);
        assert_eq!(load("- a\n").unwrap().shape, TopShape::Other);
    }

    #[test]
    fn implicit_resolution_for_the_dumper() {
        for s in ["true", "null", "~", "12", "0x1F", "1:30", "1.5", ".inf", "2024-01-01", "<<"] {
            assert!(resolves_implicitly(s), "{s}");
        }
        for s in ["yes", "09", "hello", "1.2.3", "2024-1-1"] {
            assert!(!resolves_implicitly(s), "{s}");
        }
    }
}
