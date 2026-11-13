//! Splits a markdown note into embedding-sized chunks.
//!
//! Every length here is in UTF-16 code units, as the TypeScript chunker
//! measured them, so a note splits at the same places.

use std::sync::LazyLock;

use regex::Regex;
use sha2::{Digest, Sha256};

use crate::types::Chunk;
use crate::util::js;

/// Per-chunk configuration. Callers pass the capacity budget as
/// `chunk_size`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkerConfig {
    /// Soft max size (UTF-16 units) of one chunk. Larger sections split.
    pub chunk_size: usize,
    /// The deepest heading level that still starts a section.
    pub heading_split_depth: usize,
    /// Keep fenced code blocks whole even when oversized.
    pub preserve_code_blocks: bool,
    /// Keep `$$...$$` blocks whole even when oversized.
    pub preserve_latex_blocks: bool,
    /// Drop chunks shorter than this after trimming.
    pub min_chunk_chars: usize,
}

/// The 1000-unit fallback serves only callers without a capacity budget.
pub const DEFAULT_CHUNKER_CONFIG: ChunkerConfig = ChunkerConfig {
    chunk_size: 1000,
    heading_split_depth: 4,
    preserve_code_blocks: true,
    preserve_latex_blocks: true,
    min_chunk_chars: 50,
};

impl Default for ChunkerConfig {
    fn default() -> Self {
        DEFAULT_CHUNKER_CONFIG
    }
}

/// The default config with `chunk_size` from a capacity budget.
pub fn chunker_config_from_budget(chunk_budget_chars: usize) -> ChunkerConfig {
    ChunkerConfig {
        chunk_size: chunk_budget_chars,
        ..DEFAULT_CHUNKER_CONFIG
    }
}

struct Section {
    heading: Option<String>,
    heading_level: Option<u8>,
    content: String,
    start_line: usize,
    end_line: usize,
}

/// A code fence or LaTeX block, swapped out for a sentinel token so no
/// split can cut it.
struct Protected {
    token: String,
    text: String,
}

// Private Use Area delimiters: they never occur in real markdown, they are
// not whitespace, and each is one UTF-16 unit, so a token has a fixed
// length.
const PROTECT_OPEN: char = '\u{E000}';
const PROTECT_CLOSE: char = '\u{E001}';

static CODE_FENCE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)```.*?```").unwrap());
static LATEX_BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)\$\$.*?\$\$").unwrap());
static PROTECT_TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("\u{E000}[0-9]{4}\u{E001}").unwrap());
static FRONTMATTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A---\r?\n(?s:.*?)\r?\n---\r?\n?").unwrap());
static LINE_SPLIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\r?\n").unwrap());
static HEADING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"\A(#{{1,6}})[{ws}]+({dot}+?)[{ws}]*\z",
        ws = js::WS,
        dot = js::DOT
    ))
    .unwrap()
});
static PARAGRAPH_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{2,}").unwrap());
static SENTENCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"[^.!?]+[.!?]+([{ws}]+|\z)", ws = js::WS)).unwrap());

/// Split a markdown document into chunks:
///   1. Strip frontmatter.
///   2. Swap code fences and `$$...$$` blocks for sentinel tokens.
///   3. Cut sections at headings up to `heading_split_depth`.
///   4. Split oversized sections by paragraph, then sentence, then hard.
///   5. Restore the protected regions.
pub fn chunk_markdown(content: &str, config: &ChunkerConfig) -> Vec<Chunk> {
    if content.is_empty() {
        return Vec::new();
    }
    let stripped = strip_frontmatter(content);
    if js::trim(stripped).is_empty() {
        return Vec::new();
    }

    let (safe, regions) = protect_regions(stripped, config);
    let sections = split_by_headings(&safe, config.heading_split_depth);

    let mut out = Vec::new();
    for section in &sections {
        for piece in split_oversize(&section.content, config) {
            let restored = restore_protected(&piece, &regions);
            let restored = js::trim(&restored);
            if js::utf16_len(restored) < config.min_chunk_chars {
                continue;
            }
            let heading = section.heading.clone();
            let content_hash = sha256(&format!(
                "{} {}",
                heading.as_deref().unwrap_or(""),
                restored
            ));
            out.push(Chunk {
                chunk_index: out.len(),
                heading,
                heading_level: section.heading_level,
                content: restored.to_string(),
                content_hash,
                start_line: section.start_line,
                end_line: section.end_line,
            });
        }
    }
    out
}

/// The text fed to the embedder: the heading, a blank line, the content, so
/// queries line up with heading-anchored text.
pub fn build_chunk_embedding_text(chunk: &Chunk) -> String {
    match chunk.heading.as_deref() {
        Some(h) if !h.is_empty() => format!("{h}\n\n{}", chunk.content),
        _ => chunk.content.clone(),
    }
}

pub use crate::store::chunks::chunk_id;

fn strip_frontmatter(raw: &str) -> &str {
    // Only a document that starts with `---` has frontmatter; a later `---`
    // is a horizontal rule.
    if !raw.starts_with("---\n") && !raw.starts_with("---\r\n") {
        return raw;
    }
    match FRONTMATTER_RE.find(raw) {
        Some(m) => &raw[m.end()..],
        None => raw,
    }
}

fn protect_regions(text: &str, config: &ChunkerConfig) -> (String, Vec<Protected>) {
    let mut regions = Vec::new();
    let mut out = text.to_string();
    let mut counter = 0usize;

    let mut replace_all = |re: &Regex, out: &mut String| {
        *out = re
            .replace_all(out, |caps: &regex::Captures<'_>| {
                let token = format!("{PROTECT_OPEN}{counter:04}{PROTECT_CLOSE}");
                regions.push(Protected {
                    token: token.clone(),
                    text: caps[0].to_string(),
                });
                counter += 1;
                token
            })
            .into_owned();
    };

    if config.preserve_code_blocks {
        replace_all(&CODE_FENCE_RE, &mut out);
    }
    if config.preserve_latex_blocks {
        replace_all(&LATEX_BLOCK_RE, &mut out);
    }
    (out, regions)
}

fn restore_protected(text: &str, regions: &[Protected]) -> String {
    let mut out = text.to_string();
    for r in regions {
        while out.contains(&r.token) {
            out = out.replacen(&r.token, &r.text, 1);
        }
    }
    out
}

/// Walk the document line by line and start a new section at every heading
/// at or above `max_depth`. Deeper headings stay in the enclosing section.
fn split_by_headings(text: &str, max_depth: usize) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut cursor = Section {
        heading: None,
        heading_level: None,
        content: String::new(),
        start_line: 1,
        end_line: 1,
    };
    let mut buf: Vec<&str> = Vec::new();
    let mut line_no = 0usize;

    fn flush(cursor: Section, buf: &mut Vec<&str>, end_line: usize, out: &mut Vec<Section>) {
        let mut cursor = cursor;
        cursor.content = buf.join("\n");
        cursor.end_line = end_line;
        let has_heading = cursor.heading.as_deref().is_some_and(|h| !h.is_empty());
        if !js::trim(&cursor.content).is_empty() || has_heading {
            out.push(cursor);
        }
        buf.clear();
    }

    for line in LINE_SPLIT_RE.split(text) {
        line_no += 1;
        if let Some(m) = HEADING_RE.captures(line)
            && m[1].len() <= max_depth
        {
            let prev = std::mem::replace(
                &mut cursor,
                Section {
                    heading: Some(js::trim(&m[2]).to_string()),
                    heading_level: Some(m[1].len() as u8),
                    content: String::new(),
                    start_line: line_no,
                    end_line: line_no,
                },
            );
            flush(prev, &mut buf, line_no - 1, &mut sections);
            // The heading line is not body content: the embedding text puts
            // the heading in front of the content.
            continue;
        }
        buf.push(line);
    }
    flush(cursor, &mut buf, line_no, &mut sections);
    sections
}

/// Split a section that exceeds `chunk_size`: paragraph break, then
/// sentence, then hard cut. Sentinel tokens never split.
fn split_oversize(text: &str, config: &ChunkerConfig) -> Vec<String> {
    if js::utf16_len(text) <= config.chunk_size {
        return vec![text.to_string()];
    }

    let paragraphs: Vec<&str> = PARAGRAPH_RE.split(text).collect();
    if paragraphs.len() > 1 {
        return pack_pieces(&paragraphs, "\n\n", config);
    }

    // Sentences keep their terminator.
    let sentences = split_into_sentences(text);
    if sentences.len() > 1 {
        let refs: Vec<&str> = sentences.iter().map(String::as_str).collect();
        return pack_pieces(&refs, " ", config);
    }

    hard_cut(text, config.chunk_size)
}

/// Greedily pack small pieces up to `chunk_size`. A pack that is still too
/// big is split again.
fn pack_pieces(pieces: &[&str], glue: &str, config: &ChunkerConfig) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut acc = String::new();
    for piece in pieces {
        let candidate = if acc.is_empty() {
            piece.to_string()
        } else {
            format!("{acc}{glue}{piece}")
        };
        if js::utf16_len(&candidate) <= config.chunk_size {
            acc = candidate;
        } else {
            if !acc.is_empty() {
                out.push(acc);
            }
            acc = piece.to_string();
        }
    }
    if !acc.is_empty() {
        out.push(acc);
    }

    let mut result = Vec::new();
    for chunk in out {
        if js::utf16_len(&chunk) <= config.chunk_size {
            result.push(chunk);
        } else {
            result.extend(split_oversize(&chunk, config));
        }
    }
    result
}

fn split_into_sentences(text: &str) -> Vec<String> {
    let mut matches: Vec<String> = SENTENCE_RE
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect();
    if matches.is_empty() {
        return vec![text.to_string()];
    }
    // The tail starts at the summed match length, not the last match end,
    // exactly as `text.slice(joined.length)` did.
    let joined_len: usize = matches.iter().map(|m| js::utf16_len(m)).sum();
    let tail = js::utf16_slice(text, joined_len, js::utf16_len(text));
    if !js::trim(&tail).is_empty() {
        matches.push(tail);
    }
    matches
        .iter()
        .map(|s| js::trim(s).to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Cut every `size` UTF-16 units, but never through a sentinel token: the
/// cut moves to just before the token, or past it when that would make no
/// progress.
fn hard_cut(text: &str, size: usize) -> Vec<String> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let tokens: Vec<(usize, usize)> = PROTECT_TOKEN_RE
        .find_iter(text)
        .map(|m| {
            let start = js::utf16_len(&text[..m.start()]);
            (start, start + js::utf16_len(m.as_str()))
        })
        .collect();

    let mut out = Vec::new();
    let mut i = 0usize;
    while i < units.len() {
        let mut end = (i + size).min(units.len());
        for &(start, tok_end) in &tokens {
            if start < end && tok_end > end {
                end = if start > i { start } else { tok_end };
            }
        }
        if end > i {
            out.push(String::from_utf16_lossy(&units[i..end]));
        }
        i = end;
    }
    out
}

fn sha256(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}
