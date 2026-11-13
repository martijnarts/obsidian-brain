//! JavaScript semantics the port has to reproduce: number formatting,
//! `String.prototype.trim` whitespace, `JSON.stringify`, UTF-16 offsets,
//! and the `process.env` number parsing of `Number()` and `parseInt()`.

use std::collections::HashMap;

use serde_json::Value;

/// The JS `\s` class, for use inside a regex character class.
pub const WS: &str = r"\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";

/// The JS `.` class without the `s` flag: anything but a line terminator.
pub const DOT: &str = r"[^\n\r\x{2028}\x{2029}]";

/// The characters `String.prototype.trim` and `\s` treat as whitespace.
/// Unlike `char::is_whitespace`, this includes U+FEFF and excludes U+0085.
pub fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// `s.trim()`.
pub fn trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// `s.trimStart()`.
pub fn trim_start(s: &str) -> &str {
    s.trim_start_matches(is_js_whitespace)
}

/// `s.trimEnd()`.
pub fn trim_end(s: &str) -> &str {
    s.trim_end_matches(is_js_whitespace)
}

/// `s.length`: the length in UTF-16 code units.
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// The byte offset of UTF-16 offset `units` in `s`, clamped to `s.len()`.
/// An offset inside a surrogate pair rounds down to the character start.
pub fn utf16_to_byte(s: &str, units: usize) -> usize {
    let mut seen = 0;
    for (i, c) in s.char_indices() {
        let next = seen + c.len_utf16();
        if next > units {
            return i;
        }
        seen = next;
    }
    s.len()
}

/// The UTF-16 offset of byte offset `byte` in `s`.
pub fn byte_to_utf16(s: &str, byte: usize) -> usize {
    utf16_len(&s[..byte.min(s.len())])
}

/// `String(x)` for a number: the shortest digits that round-trip, laid out
/// as ECMAScript `Number::toString` does.
pub fn number_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    // `{:e}` prints the shortest round-trip digits, like `1.2345e19`.
    let sci = format!("{:e}", x.abs());
    let (mantissa, exp) = sci
        .split_once('e')
        .expect("LowerExp always has an exponent");
    let exp: i64 = exp.parse().expect("LowerExp exponent is an integer");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i64;
    let n = exp + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        let (a, b) = digits.split_at(n as usize);
        format!("{a}.{b}")
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let sign = if e >= 0 { '+' } else { '-' };
        if k == 1 {
            format!("{digits}e{sign}{}", e.abs())
        } else {
            let (a, b) = digits.split_at(1);
            format!("{a}.{b}e{sign}{}", e.abs())
        }
    };
    if x < 0.0 { format!("-{body}") } else { body }
}

/// A JS number as a JSON value. Integral values become JSON integers, so
/// serde prints them as `JSON.stringify` does; NaN and infinities become
/// `null`, as `JSON.stringify` makes them.
pub fn number_value(x: f64) -> Value {
    if !x.is_finite() {
        return Value::Null;
    }
    if x.fract() == 0.0 && x.abs() < 1e21 {
        // The JS digits, so `12345678901234567890` keeps JS's rounding.
        let s = number_to_string(x);
        if let Ok(i) = s.parse::<i64>() {
            return Value::from(i);
        }
        if let Ok(u) = s.parse::<u64>() {
            return Value::from(u);
        }
    }
    serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
}

/// The value of a JSON number as a JS number.
pub fn as_f64(n: &serde_json::Number) -> f64 {
    n.as_f64().unwrap_or(0.0)
}

/// `String(v)` for a JSON value.
pub fn to_js_string(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => number_to_string(as_f64(n)),
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                other => to_js_string(other),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// `JSON.stringify(v)`: compact, with JS number formatting and JS key order.
pub fn json_stringify(v: &Value) -> String {
    write_js_json(v, serde_json::ser::CompactFormatter)
}

/// `JSON.stringify(v, null, 2)`.
pub fn json_stringify_pretty(v: &Value) -> String {
    write_js_json(v, serde_json::ser::PrettyFormatter::new())
}

fn write_js_json<F: serde_json::ser::Formatter>(v: &Value, inner: F) -> String {
    use serde::Serialize;
    let mut out = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut out, JsFormatter(inner));
    JsOrder(v)
        .serialize(&mut ser)
        .expect("a JSON value serializes");
    String::from_utf8(out).expect("serde_json writes UTF-8")
}

/// A value serialized with its object keys in JS order: array-index keys
/// first in ascending order, then the rest in insertion order.
struct JsOrder<'a>(&'a Value);

impl serde::Serialize for JsOrder<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq};
        match self.0 {
            Value::Array(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(&JsOrder(item))?;
                }
                seq.end()
            }
            Value::Object(map) => {
                let mut index: Vec<(u64, &String, &Value)> = map
                    .iter()
                    .filter(|(k, _)| is_array_index(k))
                    .map(|(k, v)| (k.parse().unwrap_or(0), k, v))
                    .collect();
                index.sort_by_key(|(n, _, _)| *n);
                let mut out = s.serialize_map(Some(map.len()))?;
                for (_, k, v) in index {
                    out.serialize_entry(k, &JsOrder(v))?;
                }
                for (k, v) in map.iter().filter(|(k, _)| !is_array_index(k)) {
                    out.serialize_entry(k, &JsOrder(v))?;
                }
                out.end()
            }
            other => other.serialize(s),
        }
    }
}

/// A serde_json formatter that writes numbers as `JSON.stringify` does:
/// the digits of [`number_to_string`], and integers past 2^53 rounded to
/// the nearest double first.
struct JsFormatter<F>(F);

const MAX_EXACT: u64 = 1 << 53;

impl<F: serde_json::ser::Formatter> JsFormatter<F> {
    fn number<W: ?Sized + std::io::Write>(&mut self, w: &mut W, x: f64) -> std::io::Result<()> {
        if x.is_finite() {
            w.write_all(number_to_string(x).as_bytes())
        } else {
            w.write_all(b"null")
        }
    }
}

macro_rules! delegate {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {
        $(
            fn $name<W: ?Sized + std::io::Write>(&mut self, w: &mut W $(, $arg: $ty)*) -> std::io::Result<()> {
                self.0.$name(w $(, $arg)*)
            }
        )*
    };
}

impl<F: serde_json::ser::Formatter> serde_json::ser::Formatter for JsFormatter<F> {
    fn write_i64<W: ?Sized + std::io::Write>(&mut self, w: &mut W, v: i64) -> std::io::Result<()> {
        if v.unsigned_abs() <= MAX_EXACT {
            self.0.write_i64(w, v)
        } else {
            self.number(w, v as f64)
        }
    }

    fn write_u64<W: ?Sized + std::io::Write>(&mut self, w: &mut W, v: u64) -> std::io::Result<()> {
        if v <= MAX_EXACT {
            self.0.write_u64(w, v)
        } else {
            self.number(w, v as f64)
        }
    }

    fn write_i128<W: ?Sized + std::io::Write>(
        &mut self,
        w: &mut W,
        v: i128,
    ) -> std::io::Result<()> {
        self.number(w, v as f64)
    }

    fn write_u128<W: ?Sized + std::io::Write>(
        &mut self,
        w: &mut W,
        v: u128,
    ) -> std::io::Result<()> {
        self.number(w, v as f64)
    }

    fn write_f32<W: ?Sized + std::io::Write>(&mut self, w: &mut W, v: f32) -> std::io::Result<()> {
        self.number(w, f64::from(v))
    }

    fn write_f64<W: ?Sized + std::io::Write>(&mut self, w: &mut W, v: f64) -> std::io::Result<()> {
        self.number(w, v)
    }

    delegate! {
        begin_array();
        end_array();
        begin_array_value(first: bool);
        end_array_value();
        begin_object();
        end_object();
        begin_object_key(first: bool);
        end_object_key();
        begin_object_value();
        end_object_value();
    }
}

/// ICU root order of the ASCII punctuation and symbols, which all sort
/// before digits and letters.
const ICU_PUNCT: &str = "_-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";

fn collation_key(c: char) -> (u8, u32) {
    if is_js_whitespace(c) {
        (0, c as u32)
    } else if let Some(i) = ICU_PUNCT.find(c) {
        (1, i as u32)
    } else if c.is_ascii_digit() {
        (2, c as u32)
    } else {
        (3, c.to_lowercase().next().unwrap_or(c) as u32)
    }
}

/// An approximation of `a.localeCompare(b, 'en')` (ICU collation): spaces,
/// then punctuation in ICU order, then digits, then letters compared
/// without case; on a tie, lowercase sorts before uppercase. Accents are
/// not folded, so `é` sorts after `z` instead of next to `e`.
pub fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    let primary = |s: &str| s.chars().map(collation_key).collect::<Vec<_>>();
    primary(a).cmp(&primary(b)).then_with(|| {
        let tertiary = |s: &str| s.chars().map(|c| !c.is_lowercase()).collect::<Vec<_>>();
        tertiary(a).cmp(&tertiary(b))
    })
}

/// True for a key a JS object orders first: a canonical array index.
fn is_array_index(key: &str) -> bool {
    if key == "0" {
        return true;
    }
    !key.is_empty()
        && !key.starts_with('0')
        && key.len() <= 10
        && key.bytes().all(|b| b.is_ascii_digit())
        && key.parse::<u64>().is_ok_and(|n| n < u32::MAX as u64)
}

/// Reorder `map` the way a JS object orders its keys: array-index keys
/// first in ascending order, then every other key in insertion order.
pub fn js_object_order(map: &mut serde_json::Map<String, Value>) {
    if !map.keys().any(|k| is_array_index(k)) {
        return;
    }
    let entries: Vec<(String, Value)> = std::mem::take(map).into_iter().collect();
    let (mut index, rest): (Vec<_>, Vec<_>) =
        entries.into_iter().partition(|(k, _)| is_array_index(k));
    index.sort_by_key(|(k, _)| k.parse::<u64>().unwrap_or(0));
    map.extend(index);
    map.extend(rest);
}

/// `s.slice(start, end)` on UTF-16 indices. A cut through a surrogate
/// pair yields U+FFFD, which is what Node writes for a lone surrogate when
/// the string leaves JS as UTF-8.
pub fn utf16_slice(s: &str, start: usize, end: usize) -> String {
    let units: Vec<u16> = s.encode_utf16().collect();
    let end = end.min(units.len());
    let start = start.min(end);
    String::from_utf16_lossy(&units[start..end])
}

/// `new Date(ms).toISOString()` for a whole number of milliseconds;
/// years outside 0 to 9999 get a sign and six digits.
pub fn iso_string(ms: i64) -> String {
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

/// `new Date(ms).toISOString()`; the fraction of a millisecond is dropped.
/// `None` where JS throws: NaN and times past the `Date` range.
pub fn iso_from_ms(ms: f64) -> Option<String> {
    (ms.abs() <= 8.64e15).then(|| iso_string(ms.trunc() as i64))
}

/// `date.toISOString()` for a file time, as `fs.Stats` gives it.
pub fn iso_time(t: std::time::SystemTime) -> String {
    let ms = match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64() * 1000.0,
        Err(e) => -(e.duration().as_secs_f64() * 1000.0),
    };
    iso_from_ms(ms).unwrap_or_default()
}

/// JS `<` on strings, and the default `sort()` order: by UTF-16 code units.
pub fn utf16_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// `s.slice(0, max)` on UTF-16 units, as a `&str`: a cut inside a
/// surrogate pair drops the whole character.
pub fn utf16_prefix(s: &str, max: usize) -> &str {
    &s[..utf16_to_byte(s, max)]
}

/// `path.posix.basename(p, '.md')`.
pub fn basename_md(p: &str) -> &str {
    basename(p, ".md")
}

/// `path.posix.basename(p, ext)`: the last segment, trailing slashes
/// ignored, without `ext` unless `ext` is all of it.
pub fn basename<'a>(p: &'a str, ext: &str) -> &'a str {
    let trimmed = p.trim_end_matches('/');
    let base = trimmed.rsplit('/').next().unwrap_or(trimmed);
    match base.strip_suffix(ext) {
        Some(stem) if !stem.is_empty() && !ext.is_empty() => stem,
        _ => base,
    }
}

/// The process environment as a map, the shape of `process.env`.
pub fn process_env() -> HashMap<String, String> {
    std::env::vars().collect()
}

/// A non-empty, trimmed env value.
pub fn env_value<'a>(env: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    env.get(key).map(|v| trim(v)).filter(|v| !v.is_empty())
}

/// `Number(raw)` for an env value: `Number('')` is 0, junk is NaN, and an
/// unset value (`Number(undefined)`) is NaN.
pub fn number(raw: Option<&str>) -> f64 {
    match raw {
        None => f64::NAN,
        Some(s) => {
            let t = trim(s);
            if t.is_empty() {
                0.0
            } else {
                t.parse::<f64>().unwrap_or(f64::NAN)
            }
        }
    }
}

/// `parseInt(raw, 10)`: leading digits after optional whitespace and sign.
pub fn parse_int(raw: Option<&str>) -> Option<i64> {
    let s = trim(raw?);
    let (sign, rest) = match s.as_bytes().first() {
        Some(b'-') => (-1, &s[1..]),
        Some(b'+') => (1, &s[1..]),
        _ => (1, s),
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<i64>().ok().map(|n| sign * n)
}

/// Milliseconds since the epoch, as `Date.now()`.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn number_to_string_matches_js() {
        let cases = [
            (1.0, "1"),
            (-1.5, "-1.5"),
            (0.1, "0.1"),
            (1e21, "1e+21"),
            (1e-7, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (123456789012345680000.0, "123456789012345680000"),
            (12345678901234567890.0, "12345678901234567000"),
            (0.000001, "0.000001"),
            (2.5e25, "2.5e+25"),
            (f64::NAN, "NaN"),
            (-0.0, "0"),
        ];
        for (x, want) in cases {
            assert_eq!(number_to_string(x), want, "{x}");
        }
    }

    #[test]
    fn number_value_keeps_js_digits() {
        assert_eq!(number_value(1.0), json!(1));
        assert_eq!(number_value(-0.0), json!(0));
        assert_eq!(number_value(f64::NAN), Value::Null);
        assert_eq!(
            json_stringify(&number_value(12345678901234567890.0)),
            "12345678901234567000"
        );
        assert_eq!(number_value(1.5), json!(1.5));
    }

    #[test]
    fn json_stringify_is_compact() {
        assert_eq!(
            json_stringify(&json!({"a": [1, 2.5, null], "b": "x\"\u{0}"})),
            r#"{"a":[1,2.5,null],"b":"x\"\u0000"}"#
        );
    }

    /// Expected texts captured from Node's `JSON.stringify`.
    #[test]
    fn json_stringify_numbers_match_node() {
        let floats = [
            (1.9169625785304565e-6, "0.0000019169625785304565"),
            (1e0, "1"),
            (-1e0, "-1"),
            (5e-1, "0.5"),
            (1e21, "1e+21"),
            (1e-7, "1e-7"),
            (9.999999999999999e20, "999999999999999900000"),
            (1.5e-7, "1.5e-7"),
            (1.23456e2, "123.456"),
            (0.0, "0"),
            (-0.0, "0"),
            (3.0000000000000004e-1, "0.30000000000000004"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (1e-6, "0.000001"),
            (1.2345678901234568e20, "123456789012345680000"),
            (9.007199254740992e15, "9007199254740992"),
            (1.8446744073709552e19, "18446744073709552000"),
            (-1e-7, "-1e-7"),
            (1.234e-6, "0.000001234"),
        ];
        for (x, want) in floats {
            let v = Value::Number(serde_json::Number::from_f64(x).unwrap());
            assert_eq!(json_stringify(&v), want, "{x:e}");
            assert_eq!(json_stringify_pretty(&v), want, "{x:e}");
        }
        assert_eq!(json_stringify(&json!(u64::MAX)), "18446744073709552000");
        assert_eq!(
            json_stringify(&json!(9007199254740993i64)),
            "9007199254740992"
        );
        assert_eq!(
            json_stringify(&json!(-9007199254740992i64)),
            "-9007199254740992"
        );
        assert_eq!(json_stringify(&json!(42)), "42");
    }

    /// `JSON.stringify(v, null, 2)` in Node, for the same value.
    #[test]
    fn json_stringify_pretty_matches_node() {
        let v = json!({
            "b": [1.5, {"2": "x", "a": null, "1": true}],
            "s": "\u{2028}\u{2029}\u{7f}\u{1}\u{1f}\u{8}\t\"\\/é😀",
            "e": [],
            "o": {},
        });
        let want = "{\n  \"b\": [\n    1.5,\n    {\n      \"1\": true,\n      \"2\": \"x\",\n      \"a\": null\n    }\n  ],\n  \"s\": \"\u{2028}\u{2029}\u{7f}\\u0001\\u001f\\b\\t\\\"\\\\/é😀\",\n  \"e\": [],\n  \"o\": {}\n}";
        assert_eq!(json_stringify_pretty(&v), want);
    }

    #[test]
    fn object_order_puts_index_keys_first() {
        let mut m = json!({"b": 1, "2": 2, "a": 3, "1": 4, "01": 5})
            .as_object()
            .unwrap()
            .clone();
        js_object_order(&mut m);
        assert_eq!(m.keys().collect::<Vec<_>>(), ["1", "2", "b", "a", "01"]);
    }

    #[test]
    fn locale_compare_approximates_icu() {
        use std::cmp::Ordering::*;
        assert_eq!(locale_compare("a", "B"), Less);
        assert_eq!(locale_compare("a", "A"), Less);
        assert_eq!(locale_compare("a_b", "a-b"), Less);
        assert_eq!(locale_compare("a/b", "a1"), Less);
        assert_eq!(locale_compare("10", "9"), Less);
    }

    #[test]
    fn utf16_offsets() {
        let s = "a🎉b";
        assert_eq!(utf16_len(s), 4);
        assert_eq!(utf16_to_byte(s, 1), 1);
        assert_eq!(utf16_to_byte(s, 2), 1);
        assert_eq!(utf16_to_byte(s, 3), 5);
        assert_eq!(byte_to_utf16(s, 5), 3);
        assert_eq!(trim("\u{feff} x \u{a0}"), "x");
        assert_eq!(trim("\u{85}x"), "\u{85}x");
    }

    #[test]
    fn iso_strings_match_date_to_iso_string() {
        assert_eq!(iso_string(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_string(-1), "1969-12-31T23:59:59.999Z");
        assert_eq!(iso_string(253402300800000), "+010000-01-01T00:00:00.000Z");
        assert_eq!(
            iso_from_ms(1.9).as_deref(),
            Some("1970-01-01T00:00:00.001Z")
        );
        assert_eq!(iso_from_ms(f64::NAN), None);
        assert_eq!(iso_from_ms(8.64e15 + 1.0), None);
    }

    #[test]
    fn basename_is_node_posix_basename() {
        assert_eq!(basename("a/b.md", ".md"), "b");
        assert_eq!(basename("b.md", ".md"), "b");
        assert_eq!(basename("a/.md", ".md"), ".md");
        assert_eq!(basename("a/b/", ""), "b");
        assert_eq!(basename("a\\b.md", ".md"), "a\\b");
        assert_eq!(utf16_prefix("a🎉b", 2), "a");
        assert_eq!(utf16_prefix("a🎉b", 3), "a🎉");
    }

    #[test]
    fn utf16_slice_replaces_a_cut_surrogate() {
        assert_eq!(utf16_slice("a😀b", 0, 2), "a\u{FFFD}");
        assert_eq!(utf16_slice("a😀b", 1, 3), "😀");
    }

    #[test]
    fn js_number_and_parse_int() {
        assert_eq!(number(Some("")), 0.0);
        assert!(number(Some("banana")).is_nan());
        assert!(number(None).is_nan());
        assert_eq!(number(Some(" 42 ")), 42.0);
        assert_eq!(parse_int(Some("200abc")), Some(200));
        assert_eq!(parse_int(Some("abc")), None);
        assert_eq!(parse_int(Some("-5")), Some(-5));
    }
}
