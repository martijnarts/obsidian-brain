//! Locate parts of a raw note: its headings, the section under one heading,
//! the block that carries a `^blockId`, a line range, and its tasks.
//!
//! Every function takes the whole file, frontmatter included, so the 1-based
//! line numbers it reports match what an editor shows. Frontmatter and fenced
//! code blocks never yield headings, blocks or tasks.

use std::sync::LazyLock;

use anyhow::{Result, bail};
use regex::Regex;
use serde::Serialize;

use crate::util::js;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Heading {
    pub level: usize,
    pub text: String,
    /// 1-based line number in the file.
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotePart {
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingSection {
    pub heading: Heading,
    pub part: NotePart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockPart {
    pub block_id: String,
    pub part: NotePart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineSlice {
    pub total_lines: usize,
    pub part: NotePart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TaskCount {
    pub open: usize,
    pub done: usize,
}

static HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ {0,3}(#{1,6})[ \t]+(.+?)(?:[ \t]+#+)?[ \t]*$").expect("valid pattern"));
static FENCE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^ {0,3}(`{3,}|~{3,})").expect("valid pattern"));
static LIST_ITEM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(?:[-*+]|[0-9]+[.)])\s").expect("valid pattern"));
static TASK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(?:[-*+]|[0-9]+[.)])\s+\[(.)\]").expect("valid pattern"));
static BLOCK_ID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|\s)\^([A-Za-z0-9-]+)\s*$").expect("valid pattern"));

/// Split into lines on `\n` or `\r\n`; a final newline does not add an
/// empty last line.
pub fn split_lines(raw: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = raw.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    if lines.len() > 1 && lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// For each line, true when it is frontmatter, a code fence delimiter or
/// inside a fenced code block: lines that carry no markdown structure.
fn inert_lines(lines: &[&str]) -> Vec<bool> {
    let mut inert = vec![false; lines.len()];
    let mut start = 0;
    if lines.first().is_some_and(|l| js::trim_end(l) == "---") {
        let close = lines.iter().enumerate().skip(1).find(|(_, l)| {
            let t = js::trim_end(l);
            t == "---" || t == "..."
        });
        if let Some((close, _)) = close {
            inert[..=close].fill(true);
            start = close + 1;
        }
    }
    let mut fence: Option<String> = None;
    for i in start..lines.len() {
        let m = FENCE_RE.captures(lines[i]);
        match &fence {
            None => {
                if let Some(m) = m {
                    fence = Some(m[1].to_string());
                    inert[i] = true;
                }
            }
            Some(open) => {
                inert[i] = true;
                if let Some(m) = m {
                    let f = &m[1];
                    if f.as_bytes()[0] == open.as_bytes()[0] && f.len() >= open.len() && js::trim(lines[i]) == f {
                        fence = None;
                    }
                }
            }
        }
    }
    inert
}

pub fn extract_headings(raw: &str) -> Vec<Heading> {
    let lines = split_lines(raw);
    let inert = inert_lines(&lines);
    lines
        .iter()
        .enumerate()
        .filter(|(i, _)| !inert[*i])
        .filter_map(|(i, line)| {
            let m = HEADING_RE.captures(line)?;
            Some(Heading { level: m[1].len(), text: js::trim(&m[2]).to_string(), line: i + 1 })
        })
        .collect()
}

pub fn count_tasks(raw: &str) -> TaskCount {
    let lines = split_lines(raw);
    let inert = inert_lines(&lines);
    let mut count = TaskCount { open: 0, done: 0 };
    for (i, line) in lines.iter().enumerate() {
        if inert[i] {
            continue;
        }
        if let Some(m) = TASK_RE.captures(line) {
            if &m[1] == " " {
                count.open += 1;
            } else {
                count.done += 1;
            }
        }
    }
    count
}

/// The section under a heading: the heading line down to the line before the
/// next heading of the same or a higher level, or the end of the file.
///
/// `query` matches a heading's text exactly, then case-insensitively. When
/// neither matches and `query` holds `>`, it is read as a nested path such as
/// `Project > Notes`, where each segment must sit under the one before it.
pub fn find_heading_section(raw: &str, query: &str) -> Result<HeadingSection> {
    let lines = split_lines(raw);
    let headings = extract_headings(raw);
    let section_end = |h: &Heading| -> usize {
        headings.iter().find(|o| o.line > h.line && o.level <= h.level).map_or(lines.len(), |next| next.line - 1)
    };

    let mut heading = pick_heading(&headings, js::trim(query), "the note")?;
    if heading.is_none() && query.contains('>') {
        let segments = query.split('>').map(js::trim).filter(|s| !s.is_empty());
        let mut scope: Vec<Heading> = headings.clone();
        let mut scope_name = "the note".to_string();
        for segment in segments {
            let Some(found) = pick_heading(&scope, segment, &scope_name)? else {
                bail!("Heading \"{segment}\" not found under {scope_name}");
            };
            let end = section_end(&found);
            scope = headings.iter().filter(|h| h.line > found.line && h.line <= end && h.level > found.level).cloned().collect();
            scope_name = format!("\"{}\"", found.text);
            heading = Some(found);
        }
    }
    let Some(heading) = heading else {
        bail!("Heading \"{query}\" not found");
    };

    let end_line = section_end(&heading);
    let content = lines[heading.line - 1..end_line].join("\n");
    Ok(HeadingSection { part: NotePart { start_line: heading.line, end_line, content }, heading })
}

fn pick_heading(candidates: &[Heading], text: &str, scope_name: &str) -> Result<Option<Heading>> {
    let mut hits: Vec<&Heading> = candidates.iter().filter(|h| h.text == text).collect();
    if hits.is_empty() {
        let lower = text.to_lowercase();
        hits = candidates.iter().filter(|h| h.text.to_lowercase() == lower).collect();
    }
    if hits.len() > 1 {
        let at = hits.iter().map(|h| format!("line {}", h.line)).collect::<Vec<_>>().join(", ");
        bail!("Heading \"{text}\" is ambiguous in {scope_name} ({at}). Use a nested path such as \"Parent > {text}\".");
    }
    Ok(hits.first().map(|h| (*h).clone()))
}

/// The block that carries `^blockId`: the list item it ends, or the paragraph
/// it ends. A block id alone on its own line marks the block above it (a
/// table, a quote), and that block includes the id line.
pub fn find_block(raw: &str, block_id: &str) -> Result<BlockPart> {
    let id = js::trim(block_id).trim_start_matches('^');
    if id.is_empty() {
        bail!("blockId is empty");
    }
    let lines = split_lines(raw);
    let inert = inert_lines(&lines);

    let Some(at) = lines
        .iter()
        .enumerate()
        .position(|(i, line)| !inert[i] && BLOCK_ID_RE.captures(line).is_some_and(|m| &m[1] == id))
    else {
        bail!("Block \"^{id}\" not found");
    };

    let is_boundary = |i: usize| inert[i] || js::trim(lines[i]).is_empty() || HEADING_RE.is_match(lines[i]);

    let mut start = at;
    if js::trim(lines[at]) == format!("^{id}") {
        let mut i = at as isize - 1;
        while i >= 0 && js::trim(lines[i as usize]).is_empty() && !inert[i as usize] {
            i -= 1;
        }
        while i >= 0 && !is_boundary(i as usize) {
            start = i as usize;
            i -= 1;
        }
    } else if !LIST_ITEM_RE.is_match(lines[at]) {
        while start > 0 && !is_boundary(start - 1) && !LIST_ITEM_RE.is_match(lines[start - 1]) {
            start -= 1;
        }
    }

    Ok(BlockPart {
        block_id: id.to_string(),
        part: NotePart { start_line: start + 1, end_line: at + 1, content: lines[start..=at].join("\n") },
    })
}

/// Lines `start_line`..`end_line`, 1-based and inclusive; `end_line` is
/// clamped to the file.
pub fn slice_lines(raw: &str, start_line: usize, end_line: Option<usize>) -> Result<LineSlice> {
    let lines = split_lines(raw);
    let total = lines.len();
    if start_line > total {
        bail!("startLine {start_line} is past the end of the note ({total} lines)");
    }
    let end = end_line.unwrap_or(total).min(total);
    if end < start_line {
        let shown = end_line.map_or("undefined".to_string(), |e| e.to_string());
        bail!("endLine {shown} is before startLine {start_line}");
    }
    let from = start_line.saturating_sub(1);
    Ok(LineSlice {
        total_lines: total,
        part: NotePart { start_line, end_line: end, content: lines[from..end].join("\n") },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "---\ntitle: x\n# not a heading\n---\n# Top\n\nintro ^intro\n\n## Project\n- [ ] open\n- [x] done\n```\n# code\n- [ ] code task\n```\n### Notes\ntext\n## Other\n### Notes\nmore\n| a | b |\n| - | - |\n^table\n";

    #[test]
    fn headings_skip_frontmatter_and_code() {
        let hs = extract_headings(NOTE);
        let got: Vec<_> = hs.iter().map(|h| (h.level, h.text.as_str(), h.line)).collect();
        assert_eq!(got, vec![(1, "Top", 5), (2, "Project", 9), (3, "Notes", 16), (2, "Other", 18), (3, "Notes", 19)]);
        assert_eq!(extract_headings("## Closed ##\n")[0].text, "Closed");
    }

    #[test]
    fn counts_tasks_outside_code() {
        assert_eq!(count_tasks(NOTE), TaskCount { open: 1, done: 1 });
    }

    #[test]
    fn finds_sections_by_text_and_by_path() {
        let s = find_heading_section(NOTE, "project").unwrap();
        assert_eq!((s.part.start_line, s.part.end_line), (9, 17));
        let err = find_heading_section(NOTE, "Notes").unwrap_err().to_string();
        assert_eq!(err, "Heading \"Notes\" is ambiguous in the note (line 16, line 19). Use a nested path such as \"Parent > Notes\".");
        let s = find_heading_section(NOTE, "Other > Notes").unwrap();
        assert_eq!((s.part.start_line, s.part.end_line), (19, 23));
        assert_eq!(
            find_heading_section(NOTE, "Other > Missing").unwrap_err().to_string(),
            "Heading \"Missing\" not found under \"Other\""
        );
        assert_eq!(find_heading_section(NOTE, "Nope").unwrap_err().to_string(), "Heading \"Nope\" not found");
    }

    #[test]
    fn finds_blocks() {
        let b = find_block(NOTE, "^intro").unwrap();
        assert_eq!((b.part.start_line, b.part.end_line, b.part.content.as_str()), (7, 7, "intro ^intro"));
        let t = find_block(NOTE, "table").unwrap();
        // The paragraph line above the table has no blank line between them.
        assert_eq!((t.part.start_line, t.part.end_line), (20, 23));
        assert_eq!(find_block(NOTE, "zzz").unwrap_err().to_string(), "Block \"^zzz\" not found");
        assert_eq!(find_block(NOTE, " ^ ").unwrap_err().to_string(), "blockId is empty");
    }

    #[test]
    fn slices_lines() {
        let s = slice_lines("a\r\nb\nc\n", 2, Some(9)).unwrap();
        assert_eq!((s.total_lines, s.part.end_line, s.part.content.as_str()), (3, 3, "b\nc"));
        assert_eq!(slice_lines("a\n", 5, None).unwrap_err().to_string(), "startLine 5 is past the end of the note (1 lines)");
        assert_eq!(slice_lines("a\nb\nc", 3, Some(2)).unwrap_err().to_string(), "endLine 2 is before startLine 3");
    }
}
