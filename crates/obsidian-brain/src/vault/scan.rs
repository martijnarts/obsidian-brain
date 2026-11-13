//! Helpers shared by the vault maintenance tools (`find_broken_links`,
//! `find_orphaned_notes`, `search_and_replace`, `rename_tag`,
//! `rename_heading`). They work on raw file text so a rewrite can keep every
//! byte it does not mean to change.
//!
//! Every offset here is a byte offset into the scanned string, so a caller
//! can splice with `&text[start..end]`. Masking keeps the byte length, so
//! an offset found in a mask is valid in the original text.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use rusqlite::Connection;

use super::vault_path::{in_folder, normalize_folder};
use crate::util::js;

/// Every indexed real note (stubs excluded) under `folder` and outside every
/// `exclude_folders` entry, sorted by path.
pub fn list_note_paths(
    conn: &Connection,
    folder: Option<&str>,
    exclude_folders: &[String],
) -> Result<Vec<String>> {
    let root = normalize_folder(folder)?;
    let mut excluded = Vec::new();
    for f in exclude_folders {
        let f = normalize_folder(Some(f))?;
        if !f.is_empty() {
            excluded.push(f);
        }
    }
    let mut stmt =
        conn.prepare("SELECT id FROM nodes WHERE id NOT LIKE '\\_stub/%' ESCAPE '\\' ORDER BY id")?;
    let ids = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids
        .into_iter()
        .filter(|id| in_folder(id, &root) && !excluded.iter().any(|f| in_folder(id, f)))
        .collect())
}

/// A note split into its YAML frontmatter block (fences included) and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontmatterSplit<'a> {
    pub frontmatter: &'a str,
    pub body: &'a str,
}

static FRONTMATTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\A---\r?\n(?:(?s:.)*?\r?\n)?---[ \t]*(?:\r?\n|\z)").expect("valid pattern")
});

/// Split a file into its YAML frontmatter block (fences included) and body.
pub fn split_frontmatter(raw: &str) -> FrontmatterSplit<'_> {
    match FRONTMATTER.find(raw) {
        Some(m) => FrontmatterSplit {
            frontmatter: &raw[..m.end()],
            body: &raw[m.end()..],
        },
        None => FrontmatterSplit {
            frontmatter: "",
            body: raw,
        },
    }
}

/// Every character but `\n` becomes spaces, as many as its UTF-8 bytes.
fn blank(s: &str, out: &mut String) {
    for c in s.chars() {
        if c == '\n' {
            out.push('\n');
        } else {
            out.extend(std::iter::repeat_n(' ', c.len_utf8()));
        }
    }
}

// An inline code span: a backtick run closed by a run of the same length,
// never across a blank line.
static INLINE_CODE: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    fancy_regex::Regex::new(r"(?<!`)(`+)(?!`)(?:(?!\n[ \t]*\n)[\s\S])*?(?<!`)\1(?!`)")
        .expect("valid pattern")
});
// A fenced block runs to its closing fence, or to the end of the text when
// it is never closed.
static FENCE: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    fancy_regex::Regex::new(
        r"(?m)^( {0,3})(`{3,}|~{3,})[^\n]*\n[\s\S]*?(?:^ {0,3}\2[`~]*[ \t]*$|(?![\s\S]))",
    )
    .expect("valid pattern")
});

fn ranges(re: &fancy_regex::Regex, text: &str) -> Vec<(usize, usize)> {
    // A backtracking limit hit ends the scan; what was found so far stands.
    re.find_iter(text)
        .map_while(|m| m.ok())
        .map(|m| (m.start(), m.end()))
        .collect()
}

fn blank_ranges(text: &str, found: &[(usize, usize)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for &(start, end) in found {
        out.push_str(&text[at..start]);
        blank(&text[start..end], &mut out);
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

/// `[start, end)` byte offsets of every fenced code block, fences included.
pub fn fenced_code_ranges(text: &str) -> Vec<(usize, usize)> {
    ranges(&FENCE, text)
}

/// Same-length copy of `text` with fenced code blocks (and, unless
/// `fenced_only`, inline code spans) blanked to spaces. Offsets and line
/// numbers in the result match the original, so a match found in the mask
/// can be spliced into the original text.
pub fn mask_code(text: &str, fenced_only: bool) -> String {
    let masked = blank_ranges(text, &fenced_code_ranges(text));
    if fenced_only {
        return masked;
    }
    let spans = ranges(&INLINE_CODE, &masked);
    blank_ranges(&masked, &spans)
}

/// 1-based line number of a byte offset.
pub fn line_at(text: &str, offset: usize) -> usize {
    1 + text.as_bytes()[..offset.min(text.len())]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub level: usize,
    pub text: String,
    /// Byte offset of the heading text within the scanned string.
    pub start: usize,
    pub end: usize,
    /// 1-based line number.
    pub line: usize,
}

static HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?m)^(#{{1,6}})[ \t]+({}*?)(?:[ \t]+#+)?[ \t]*$",
        js::DOT
    ))
    .expect("valid pattern")
});

/// ATX headings of a note body, outside fenced code.
pub fn find_headings(body: &str) -> Vec<Heading> {
    let masked = mask_code(body, true);
    let mut out = Vec::new();
    for c in HEADING.captures_iter(&masked) {
        let whole = c.get(0).expect("group 0");
        let hashes = &c[1];
        let text = &c[2];
        if text.is_empty() {
            continue;
        }
        let rel = whole.as_str()[hashes.len()..]
            .find(text)
            .map_or(0, |i| i + hashes.len());
        let start = whole.start() + rel;
        out.push(Heading {
            level: hashes.len(),
            text: text.to_string(),
            start,
            end: start + text.len(),
            line: line_at(body, whole.start()),
        });
    }
    out
}

static BLOCK_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"(?m)(?:^|[{}])\^([A-Za-z0-9-]+)[ \t]*$", js::WS)).expect("valid pattern")
});

/// Block ids (`^id` at the end of a line) of a note body, outside fenced code.
pub fn find_block_ids(body: &str) -> BTreeSet<String> {
    BLOCK_ID
        .captures_iter(&mask_code(body, true))
        .map(|c| c[1].to_string())
        .collect()
}

static WS_RUN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("[{}]+", js::WS)).expect("valid pattern"));

/// Heading comparison key: Obsidian resolves heading links case-insensitively.
pub fn heading_key(text: &str) -> String {
    WS_RUN.replace_all(js::trim(text), " ").to_lowercase()
}

/// A wiki-link or embed in raw text. `target` is the note part, `subpath`
/// the part after the first `#` (`None` when absent), so a block reference
/// reads `^id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiLinkMatch {
    pub embed: bool,
    pub target: String,
    pub subpath: Option<String>,
    pub alias: Option<String>,
    /// Byte offsets of the whole link in the scanned text.
    pub start: usize,
    pub end: usize,
    pub raw: String,
}

static WIKI_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(!?)\[\[([^\]\n]+)\]\]").expect("valid pattern"));

/// Wiki-links and embeds outside fenced and inline code.
pub fn find_wiki_links(text: &str) -> Vec<WikiLinkMatch> {
    let masked = mask_code(text, false);
    let mut out = Vec::new();
    for c in WIKI_LINK.captures_iter(&masked) {
        let whole = c.get(0).expect("group 0");
        let inner = &c[2];
        let (link, alias) = match inner.find('|') {
            Some(p) => (&inner[..p], Some(inner[p + 1..].to_string())),
            None => (inner, None),
        };
        // `[[Note^id]]` is shorthand for `[[Note#^id]]`, as in the parser.
        let (target, subpath) = match link.find(['#', '^']) {
            None => (link, None),
            Some(cut) => {
                let from = if link.as_bytes()[cut] == b'#' {
                    cut + 1
                } else {
                    cut
                };
                (&link[..cut], Some(link[from..].to_string()))
            }
        };
        out.push(WikiLinkMatch {
            embed: &c[1] == "!",
            target: js::trim(target).to_string(),
            subpath,
            alias,
            start: whole.start(),
            end: whole.end(),
            raw: text[whole.start()..whole.end()].to_string(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_frontmatter_handles_none_empty_crlf_and_fence_at_eof() {
        let s = split_frontmatter("body");
        assert_eq!((s.frontmatter, s.body), ("", "body"));
        let s = split_frontmatter("---\n---\nx");
        assert_eq!((s.frontmatter, s.body), ("---\n---\n", "x"));
        assert_eq!(split_frontmatter("---\r\na: 1\r\n---\r\nx").body, "x");
        let s = split_frontmatter("---\na: 1\n---");
        assert_eq!((s.frontmatter, s.body), ("---\na: 1\n---", ""));
    }

    #[test]
    fn mask_code_keeps_length_and_newlines_blanks_fences_and_inline_spans() {
        let text = "a `b` c\n```js\n[[x]]\n```\n~~~\nopen fence\n";
        let masked = mask_code(text, false);
        assert_eq!(masked.len(), text.len());
        assert_eq!(masked.split('\n').count(), text.split('\n').count());
        assert!(!masked.contains('b'));
        assert!(!masked.contains("[[x]]"));
        assert!(!masked.contains("open fence"));
        assert_eq!(mask_code("x `y` z", true), "x `y` z");
        assert_eq!(mask_code("a ``b ` c`` d", false), "a           d");
    }

    #[test]
    fn mask_code_keeps_byte_offsets_of_multibyte_text() {
        let text = "é `ü` [[x]]";
        let masked = mask_code(text, false);
        assert_eq!(masked.len(), text.len());
        assert_eq!(masked.find("[[x]]"), text.find("[[x]]"));
    }

    #[test]
    fn find_headings_reads_atx_headings_with_closing_hashes_outside_code() {
        let hs = find_headings("# One\n#nope\n```\n# In code\n```\n### Three ###\n#  \n");
        let got: Vec<_> = hs
            .iter()
            .map(|h| (h.level, h.text.as_str(), h.line))
            .collect();
        assert_eq!(got, [(1, "One", 1), (3, "Three", 6)]);
        let body = "intro\n## Two words\n";
        let h = &find_headings(body)[0];
        assert_eq!(&body[h.start..h.end], "Two words");
    }

    #[test]
    fn find_wiki_links_splits_target_subpath_and_alias() {
        let links = find_wiki_links("[[N#H|al]] ![[M^b]] [[#S]]");
        let (a, b, c) = (&links[0], &links[1], &links[2]);
        assert_eq!(
            (
                a.embed,
                a.target.as_str(),
                a.subpath.as_deref(),
                a.alias.as_deref()
            ),
            (false, "N", Some("H"), Some("al"))
        );
        assert_eq!(
            (
                b.embed,
                b.target.as_str(),
                b.subpath.as_deref(),
                b.alias.as_deref()
            ),
            (true, "M", Some("^b"), None)
        );
        assert_eq!((c.target.as_str(), c.subpath.as_deref()), ("", Some("S")));
    }

    #[test]
    fn find_block_ids_and_heading_key() {
        let ids = find_block_ids("para ^abc\n```\nx ^code\n```\n^top\n");
        assert_eq!(ids.into_iter().collect::<Vec<_>>(), ["abc", "top"]);
        assert_eq!(heading_key("  My   Heading "), "my heading");
    }

    #[test]
    fn line_at_counts_newlines_before_the_offset() {
        assert_eq!(line_at("a\nb\nc", 0), 1);
        assert_eq!(line_at("a\nb\nc", 2), 2);
        assert_eq!(line_at("a\nb\nc", 4), 3);
    }
}
