//! Helpers that reproduce JavaScript string semantics: UTF-16 lengths and
//! slices, and the JS whitespace set that `trim()` and `\s` use.

use std::collections::HashMap;

/// The JS `\s` class, for use inside a regex character class.
pub const WS: &str = r"\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";

/// The JS `.` class without the `s` flag: anything but a line terminator.
pub const DOT: &str = r"[^\n\r\x{2028}\x{2029}]";

pub fn is_ws(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// `String.prototype.trim`.
pub fn trim(s: &str) -> &str {
    s.trim_matches(is_ws)
}

/// `String.prototype.length`: UTF-16 code units.
pub fn len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// `String.prototype.slice(start, end)` on UTF-16 indices. A cut through a
/// surrogate pair yields U+FFFD, which is what Node writes for a lone
/// surrogate when the string leaves JS as UTF-8.
pub fn slice(s: &str, start: usize, end: usize) -> String {
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

/// `Number(raw)` for an env value: `Number('')` is 0, junk is NaN.
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

    #[test]
    fn utf16_len_and_slice() {
        assert_eq!(len("a😀b"), 4);
        assert_eq!(slice("a😀b", 0, 2), "a\u{FFFD}");
        assert_eq!(slice("a😀b", 1, 3), "😀");
    }

    #[test]
    fn js_number_and_parse_int() {
        assert_eq!(number(Some("")), 0.0);
        assert!(number(Some("banana")).is_nan());
        assert_eq!(number(Some(" 42 ")), 42.0);
        assert_eq!(parse_int(Some("200abc")), Some(200));
        assert_eq!(parse_int(Some("abc")), None);
        assert_eq!(parse_int(Some("-5")), Some(-5));
    }

    #[test]
    fn trim_matches_js() {
        assert_eq!(trim("\u{FEFF} x \u{A0}"), "x");
        assert_eq!(trim("\u{85}x"), "\u{85}x");
    }
}
