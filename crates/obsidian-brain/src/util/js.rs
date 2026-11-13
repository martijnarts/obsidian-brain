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

/// `JSON.stringify(v)`: compact, with JS number formatting.
pub fn json_stringify(v: &Value) -> String {
    let mut out = String::new();
    write_json(&mut out, v);
    out
}

fn write_json(out: &mut String, v: &Value) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64().filter(|i| i.unsigned_abs() <= 1 << 53) {
                out.push_str(&i.to_string());
            } else {
                let x = as_f64(n);
                out.push_str(&if x.is_finite() {
                    number_to_string(x)
                } else {
                    "null".into()
                });
            }
        }
        Value::String(s) => out.push_str(&serde_json::to_string(s).expect("strings serialize")),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (k, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).expect("strings serialize"));
                out.push(':');
                write_json(out, item);
            }
            out.push('}');
        }
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
