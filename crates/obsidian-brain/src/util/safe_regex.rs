//! Compile client regexes written in JavaScript syntax with `fancy-regex`.
//!
//! Clients send JavaScript patterns and flags, so the pattern is translated
//! before it is compiled. The translation keeps the JS meaning where Rust
//! differs:
//!
//! - `\d`, `\w` and `\b` are ASCII-only, as in JS. `\b` and `\B` become
//!   lookarounds, so a pattern that uses them runs on the backtracking VM.
//! - `.` does not match `\n`, `\r`, U+2028 or U+2029 unless `s` is set.
//! - Under `m`, `^` and `$` treat `\r\n` as one line end (the `R` flag).
//! - `[^]` matches any character and `[]` matches nothing; `[`, `&` and `~`
//!   inside a class are literal; `{` that does not start a quantifier is
//!   literal; `\/`, `\0`, `\cX` and `\uXXXX` (surrogate pairs included)
//!   mean what they mean in JS; an identity escape such as `\z` is the
//!   letter.
//!
//! Gaps that remain:
//!
//! - `\s` uses Unicode White_Space: it matches U+0085 and not U+FEFF.
//! - `i` uses Unicode simple case folding. JS without `u` folds by
//!   `toUpperCase`, so a few characters (`ſ`, the Kelvin sign) differ.
//! - `^` and `$` under `m` do not see U+2028 and U+2029 as line ends, and
//!   `^` never matches between `\r` and `\n`.
//! - Without `u`, JS `.` matches one UTF-16 code unit, so it matches half an
//!   astral character; here it matches the whole character.
//! - `\D` and `\W` inside a class stay Unicode.
//! - `\1` where group 1 does not exist is an error here; JS reads it as an
//!   octal escape.
//! - `g` and `y` change how a JS caller iterates matches, not the pattern.
//!   They are reported in [`JsFlags`]; a caller that honours `y` must check
//!   that each match starts where the previous one ended.
//! - Error texts after `Invalid regex: ` come from `fancy-regex`.
//! - `fancy-regex` stops a match that backtracks more than its limit (one
//!   million steps) and returns an error from the match call.

use std::sync::LazyLock;

use anyhow::{Result, anyhow, bail};

use super::js::utf16_len;

pub const MAX_PATTERN_LENGTH: usize = 500;

/// A group that holds a quantifier and is itself quantified, like `(a+)+`,
/// `(\w*x)*` or `(a{1,})+`. Such patterns backtrack exponentially on a
/// near-miss, so they are refused up front.
static NESTED_QUANTIFIER: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"\((?:[^()\\]|\\.)*(?:[+*]|\{[0-9]+,?[0-9]*\})(?:[^()\\]|\\.)*\)(?:[+*]|\{[0-9]+,?[0-9]*\})",
    )
    .expect("valid pattern")
});

/// The flags of a JS regex.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JsFlags {
    pub has_indices: bool,
    pub global: bool,
    pub ignore_case: bool,
    pub multiline: bool,
    pub dot_all: bool,
    pub unicode: bool,
    pub unicode_sets: bool,
    pub sticky: bool,
}

impl JsFlags {
    /// Parse a JS flag string. Unknown and repeated flags are an error, as
    /// in the `RegExp` constructor.
    pub fn parse(flags: &str) -> Result<Self> {
        let mut f = JsFlags::default();
        let invalid = || anyhow!("Invalid flags supplied to RegExp constructor '{flags}'");
        for c in flags.chars() {
            let slot = match c {
                'd' => &mut f.has_indices,
                'g' => &mut f.global,
                'i' => &mut f.ignore_case,
                'm' => &mut f.multiline,
                's' => &mut f.dot_all,
                'u' => &mut f.unicode,
                'v' => &mut f.unicode_sets,
                'y' => &mut f.sticky,
                _ => return Err(invalid()),
            };
            if *slot {
                return Err(invalid());
            }
            *slot = true;
        }
        if f.unicode && f.unicode_sets {
            return Err(invalid());
        }
        Ok(f)
    }

    /// The flags in canonical order, as `RegExp.prototype.flags` gives them.
    pub fn as_js(&self) -> String {
        [
            (self.has_indices, 'd'),
            (self.global, 'g'),
            (self.ignore_case, 'i'),
            (self.multiline, 'm'),
            (self.dot_all, 's'),
            (self.unicode, 'u'),
            (self.unicode_sets, 'v'),
            (self.sticky, 'y'),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, c)| *c)
        .collect()
    }
}

/// A compiled client regex with the JS flags it was compiled from.
#[derive(Debug, Clone)]
pub struct UserRegex {
    pub regex: fancy_regex::Regex,
    pub flags: JsFlags,
}

impl UserRegex {
    /// `re.test(text)` from the start of `text`. A match that hits the
    /// backtrack limit counts as no match.
    pub fn is_match(&self, text: &str) -> bool {
        self.regex.is_match(text).unwrap_or(false)
    }
}

/// Escape regex metacharacters so `s` matches as plain text, as the JS
/// `s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')` does.
pub fn escape_regex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if ".*+?^${}()|[]\\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Compile a client regex with the guards every pattern tool applies: a
/// length cap, and no quantified group that itself holds a quantifier.
/// `literal` escapes the pattern so it matches as plain text.
pub fn compile_user_regex(pattern: &str, flags: &str, literal: bool) -> Result<UserRegex> {
    let len = utf16_len(pattern);
    if len > MAX_PATTERN_LENGTH {
        bail!("Pattern is too long ({len} chars, max {MAX_PATTERN_LENGTH}).");
    }
    if literal {
        let flags = JsFlags::parse(flags)?;
        let translated = translate(&escape_regex(pattern), flags)?;
        let regex =
            fancy_regex::Regex::new(&translated).map_err(|e| anyhow!("Invalid regex: {e}"))?;
        return Ok(UserRegex { regex, flags });
    }
    if NESTED_QUANTIFIER.is_match(pattern) {
        bail!(
            "Pattern has a quantified group that contains a quantifier, like `(a+)+`. Rewrite it without the nesting."
        );
    }
    let flags = JsFlags::parse(flags).map_err(|e| anyhow!("Invalid regex: {e}"))?;
    let translated = translate(pattern, flags).map_err(|e| anyhow!("Invalid regex: {e}"))?;
    let regex = fancy_regex::Regex::new(&translated).map_err(|e| anyhow!("Invalid regex: {e}"))?;
    Ok(UserRegex { regex, flags })
}

const WORD: &str = "0-9A-Za-z_";

/// Translate a JS pattern into `fancy-regex` syntax with the same meaning.
/// See the module docs for what changes and what does not.
pub fn translate_js_regex(pattern: &str, flags: &str) -> Result<String> {
    translate(pattern, JsFlags::parse(flags)?)
}

fn translate(pattern: &str, flags: JsFlags) -> Result<String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::from("(?");
    if flags.ignore_case {
        out.push('i');
    }
    if flags.multiline {
        out.push('m');
    }
    out.push_str("R)");
    let boundary = format!("(?:(?<=[{WORD}])(?![{WORD}])|(?<![{WORD}])(?=[{WORD}]))");
    let non_boundary = format!("(?:(?<=[{WORD}])(?=[{WORD}])|(?<![{WORD}])(?![{WORD}]))");

    let mut in_class = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                let Some(&e) = chars.get(i + 1) else {
                    bail!("\\ at end of pattern");
                };
                i += 2;
                match e {
                    'd' | 'w' if in_class => {
                        out.push_str(if e == 'd' { "0-9" } else { WORD });
                        // `[\d-z]`: a `-` after a class escape is literal in JS.
                        if chars.get(i) == Some(&'-') && chars.get(i + 1) != Some(&']') {
                            out.push_str("\\-");
                            i += 1;
                        }
                    }
                    'b' if in_class => out.push_str("\\x08"),
                    'd' => out.push_str("[0-9]"),
                    'D' if !in_class => out.push_str("[^0-9]"),
                    'w' => out.push_str(&format!("[{WORD}]")),
                    'W' if !in_class => out.push_str(&format!("[^{WORD}]")),
                    'b' => out.push_str(&boundary),
                    'B' => out.push_str(&non_boundary),
                    '0' if !chars.get(i).is_some_and(char::is_ascii_digit) => out.push_str("\\x00"),
                    'c' if chars.get(i).is_some_and(char::is_ascii_alphabetic) => {
                        let ctrl = (chars[i] as u32) % 32;
                        out.push_str(&format!("\\x{{{ctrl:02X}}}"));
                        i += 1;
                    }
                    'u' => {
                        let hex4 = |at: usize| -> Option<u32> {
                            let s: String = chars.get(at..at + 4)?.iter().collect();
                            if s.chars().all(|h| h.is_ascii_hexdigit()) {
                                u32::from_str_radix(&s, 16).ok()
                            } else {
                                None
                            }
                        };
                        if chars.get(i) == Some(&'{') && (flags.unicode || flags.unicode_sets) {
                            // `\u{...}` passes through.
                            out.push_str("\\u");
                        } else if let Some(hi) = hex4(i) {
                            i += 4;
                            let mut cp = hi;
                            if (0xD800..0xDC00).contains(&hi)
                                && chars.get(i) == Some(&'\\')
                                && chars.get(i + 1) == Some(&'u')
                                && let Some(lo) =
                                    hex4(i + 2).filter(|lo| (0xDC00..0xE000).contains(lo))
                            {
                                cp = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                                i += 6;
                            }
                            if (0xD800..0xE000).contains(&cp) {
                                bail!("a lone surrogate \\u{cp:04X} cannot match UTF-8 text");
                            }
                            out.push_str(&format!("\\x{{{cp:X}}}"));
                        } else {
                            out.push('u');
                        }
                    }
                    'x' if chars
                        .get(i..i + 2)
                        .is_some_and(|h| h.iter().all(char::is_ascii_hexdigit)) =>
                    {
                        out.push_str("\\x");
                    }
                    'D' | 'W' | 's' | 'S' | 'n' | 'r' | 't' | 'f' | 'v' | 'k' => {
                        out.push('\\');
                        out.push(e);
                    }
                    'p' | 'P' if flags.unicode || flags.unicode_sets => {
                        out.push('\\');
                        out.push(e);
                    }
                    '1'..='9' => {
                        out.push('\\');
                        out.push(e);
                    }
                    e if e.is_ascii_alphanumeric() => {
                        // An identity escape: `\z` is `z` in JS.
                        out.push(e);
                    }
                    e if e.is_ascii() => {
                        out.push('\\');
                        out.push(e);
                    }
                    e => out.push(e),
                }
                continue;
            }
            '[' if !in_class => {
                if chars.get(i + 1) == Some(&'^') && chars.get(i + 2) == Some(&']') {
                    out.push_str("(?s:.)");
                    i += 3;
                    continue;
                }
                if chars.get(i + 1) == Some(&']') {
                    out.push_str("[^\\s\\S]");
                    i += 2;
                    continue;
                }
                in_class = true;
                out.push('[');
                if chars.get(i + 1) == Some(&'^') {
                    out.push('^');
                    i += 1;
                }
            }
            '[' | '&' | '~' if in_class => {
                out.push('\\');
                out.push(c);
            }
            ']' if in_class => {
                in_class = false;
                out.push(']');
            }
            '.' if !in_class => {
                if flags.dot_all {
                    out.push_str("(?s:.)");
                } else {
                    out.push_str("[^\\n\\r\\x{2028}\\x{2029}]");
                }
            }
            '{' if !in_class => {
                if starts_quantifier(&chars[i..]) {
                    // Copy the quantifier and its closing brace verbatim.
                    let close = chars[i..]
                        .iter()
                        .position(|&ch| ch == '}')
                        .expect("checked by starts_quantifier");
                    out.extend(&chars[i..=i + close]);
                    i += close + 1;
                    continue;
                }
                out.push_str("\\{");
            }
            '}' if !in_class => out.push_str("\\}"),
            _ => out.push(c),
        }
        i += 1;
    }
    Ok(out)
}

/// True when `rest` starts with a JS quantifier: `{n}`, `{n,}` or `{n,m}`.
fn starts_quantifier(rest: &[char]) -> bool {
    let mut j = 1;
    let digits = |j: &mut usize| {
        let start = *j;
        while rest.get(*j).is_some_and(char::is_ascii_digit) {
            *j += 1;
        }
        *j > start
    };
    if !digits(&mut j) {
        return false;
    }
    if rest.get(j) == Some(&',') {
        j += 1;
        digits(&mut j);
    }
    rest.get(j) == Some(&'}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test(re: &UserRegex, s: &str) -> bool {
        re.is_match(s)
    }

    #[test]
    fn escapes_metacharacters_in_a_literal_pattern() {
        let re = compile_user_regex("a.b (c)", "", true).unwrap();
        assert!(test(&re, "xa.b (c)y"));
        assert!(!test(&re, "aXb (c)"));
        assert!(test(
            &compile_user_regex("(a+)+", "g", true).unwrap(),
            "x(a+)+y"
        ));
    }

    #[test]
    fn passes_the_flags_through() {
        assert!(test(&compile_user_regex("Foo", "i", true).unwrap(), "foo"));
        assert!(!test(&compile_user_regex("Foo", "", true).unwrap(), "foo"));
        assert_eq!(
            compile_user_regex("a", "gm", false).unwrap().flags.as_js(),
            "gm"
        );
    }

    #[test]
    fn refuses_patterns_over_500_characters() {
        let err = compile_user_regex(&"a".repeat(501), "g", false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("too long (501 chars, max 500)"), "{err}");
        assert!(
            compile_user_regex(&"a".repeat(501), "g", true)
                .unwrap_err()
                .to_string()
                .contains("too long")
        );
        assert!(compile_user_regex(&"a".repeat(500), "", false).is_ok());
    }

    #[test]
    fn refuses_quantified_groups_that_contain_a_quantifier() {
        for p in [
            "(a+)+",
            "(a+)+$",
            "(\\w*x)*",
            "(a{1,})+",
            "(a|b*){2,}",
            "(a+){3}",
        ] {
            let err = compile_user_regex(p, "g", false).unwrap_err().to_string();
            assert!(err.contains("quantified group"), "{p}: {err}");
        }
    }

    #[test]
    fn refuses_invalid_syntax() {
        for p in ["(", "["] {
            let err = compile_user_regex(p, "g", false).unwrap_err().to_string();
            assert!(err.contains("Invalid regex"), "{p}: {err}");
        }
    }

    #[test]
    fn accepts_ordinary_groups_and_escaped_parentheses() {
        assert!(test(&compile_user_regex("(a+)", "g", false).unwrap(), "aa"));
        assert!(test(
            &compile_user_regex("(foo|bar)+", "", false).unwrap(),
            "barfoo"
        ));
        assert!(test(
            &compile_user_regex("\\(a+\\)+", "", false).unwrap(),
            "(aa))"
        ));
    }

    #[test]
    fn keeps_js_meaning_where_rust_differs() {
        let c = |p: &str, f: &str| compile_user_regex(p, f, false).unwrap();
        // ASCII \d, \w and \b.
        assert!(!test(&c("^\\d$", ""), "٣"));
        assert!(!test(&c("^\\w$", ""), "é"));
        assert!(test(&c("caf\\b", ""), "café"));
        assert!(!test(&c("\\bé", ""), "é"));
        // `.` stops at \r, and `$` under m sees \r\n.
        assert!(!test(&c("^a.b$", ""), "a\rb"));
        assert!(test(&c("^a.b$", "s"), "a\rb"));
        assert!(test(&c("foo$", "m"), "foo\r\nbar"));
        // JS-only syntax.
        assert!(test(&c("a[^]b", ""), "a\nb"));
        assert!(!test(&c("a[]", ""), "a"));
        assert!(test(&c("[[]", ""), "["));
        assert!(test(&c("a{", ""), "a{"));
        assert!(test(&c("a{,2}", ""), "a{,2}"));
        assert!(test(&c("^a{2}$", ""), "aa"));
        assert!(test(&c("\\/", ""), "/"));
        assert!(test(&c("\\u0041\\x42", ""), "AB"));
        assert!(test(&c("\\uD83C\\uDF89", ""), "🎉"));
        assert!(test(&c("\\cJ", ""), "\n"));
        assert!(test(&c("\\0", ""), "\0"));
        assert!(test(&c("\\z", ""), "z"));
        assert!(test(&c("[\\d-]", ""), "-"));
        assert!(test(&c("^[\\d-z]+$", ""), "1-z"));
        // Lookbehind and backreferences.
        assert!(test(&c("(?<=a+)b", ""), "aab"));
        assert!(test(&c("(?<n>a)\\k<n>", ""), "aa"));
        assert!(test(&c("(x)\\1", ""), "xx"));
    }

    #[test]
    fn rejects_bad_flags() {
        assert!(compile_user_regex("a", "gg", false).is_err());
        assert!(compile_user_regex("a", "q", false).is_err());
    }
}
