//! `^block-id` placement, following Obsidian's rules: a paragraph takes
//! ` ^id` at the end of its last line, a list item or heading on its own
//! line, and a table, quote, callout or fenced code block takes `^id` on a
//! separate line after the block, set off by blank lines. Pure functions over
//! the file split on `\n`; the caller owns the read and the write.

use std::sync::LazyLock;

use anyhow::{Result, bail};
use regex::Regex;

use super::markdown_lines::{fence_ranges, frontmatter_end, strip_cr};
use crate::util::js;

pub static BLOCK_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9-]+$").expect("valid pattern"));
static TRAILING_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|\s)\^([A-Za-z0-9-]+)$").expect("valid pattern"));
static OWN_LINE_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\^([A-Za-z0-9-]+)$").expect("valid pattern"));
static HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^#{1,6}(?:[ \t]|$)").expect("valid pattern"));
static LIST_ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[ \t]*(?:[-*+]|[0-9]+[.)])(?:[ \t]|$)").expect("valid pattern"));
static TABLE_ROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[ \t]*\|").expect("valid pattern"));
static QUOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[ \t]*>").expect("valid pattern"));
static FENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[ \t]*(`{3,}|~{3,})").expect("valid pattern"));
static HEADING_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^#{1,6}(?:[ \t]+|$)").expect("valid pattern"));
static HEADING_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|\s)\^[A-Za-z0-9-]+$").expect("valid pattern"));
static CLOSING_HASHES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[ \t]+#+$").expect("valid pattern"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// The id goes at the end of the block's last line.
    Inline,
    /// The id goes on its own line after the block.
    OwnLine,
}

/// A block as 0-based inclusive line indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingId {
    pub id: String,
    /// 0-based line of the id.
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attached {
    pub lines: Vec<String>,
    /// 0-based line the id sits on.
    pub line: usize,
}

fn is_blank(l: &str) -> bool {
    js::trim(strip_cr(l)).is_empty()
}

fn clean(l: &str) -> &str {
    js::trim_end(strip_cr(l))
}

/// The block that 0-based line `idx` belongs to. Errors for lines that
/// cannot carry an id.
pub fn locate_block<S: AsRef<str>>(lines: &[S], idx: usize) -> Result<Block> {
    let fm_end = frontmatter_end(lines);
    if fm_end.is_some_and(|end| idx <= end) {
        bail!(
            "Line {} is in the frontmatter; a block id needs a body line",
            idx + 1
        );
    }
    if is_blank(lines[idx].as_ref()) {
        bail!(
            "Line {} is blank; a block id needs a line with content",
            idx + 1
        );
    }

    let body_start = fm_end.map_or(0, |e| e + 1);
    if let Some(&(a, b)) = fence_ranges(lines, body_start)
        .iter()
        .find(|(a, b)| idx >= *a && idx <= *b)
    {
        return Ok(Block {
            kind: BlockKind::OwnLine,
            start: a,
            end: b,
        });
    }

    let text = clean(lines[idx].as_ref());
    if HEADING.is_match(text) || LIST_ITEM.is_match(text) {
        return Ok(Block {
            kind: BlockKind::Inline,
            start: idx,
            end: idx,
        });
    }

    let extent = |test: &dyn Fn(&str) -> bool| -> (usize, usize) {
        let mut a = idx;
        let mut b = idx;
        while a > 0 && test(clean(lines[a - 1].as_ref())) {
            a -= 1;
        }
        while b + 1 < lines.len() && test(clean(lines[b + 1].as_ref())) {
            b += 1;
        }
        (a, b)
    };
    for re in [&*TABLE_ROW, &*QUOTE] {
        if re.is_match(text) {
            let (a, b) = extent(&|l| re.is_match(l));
            return Ok(Block {
                kind: BlockKind::OwnLine,
                start: a,
                end: b,
            });
        }
    }
    let plain = |l: &str| -> bool {
        !js::trim(l).is_empty()
            && ![&*HEADING, &*LIST_ITEM, &*TABLE_ROW, &*QUOTE, &*OWN_LINE_ID]
                .iter()
                .any(|re| re.is_match(l))
            && !FENCE.is_match(l)
    };
    let (a, b) = extent(&plain);
    Ok(Block {
        kind: BlockKind::Inline,
        start: a,
        end: b,
    })
}

/// The id the block already carries, with its 0-based line.
pub fn existing_block_id<S: AsRef<str>>(lines: &[S], block: Block) -> Option<ExistingId> {
    if block.kind == BlockKind::Inline {
        let m = TRAILING_ID.captures(clean(lines[block.end].as_ref()))?;
        return Some(ExistingId {
            id: m[1].to_string(),
            line: block.end,
        });
    }
    let mut i = block.end + 1;
    if i < lines.len() && is_blank(lines[i].as_ref()) {
        i += 1;
    }
    let m = OWN_LINE_ID.captures(clean(lines.get(i)?.as_ref()))?;
    Some(ExistingId {
        id: m[1].to_string(),
        line: i,
    })
}

/// Every block id written in the note, in first-seen order.
pub fn all_block_ids<S: AsRef<str>>(lines: &[S]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for l in lines {
        let l = clean(l.as_ref());
        if let Some(m) = TRAILING_ID.captures(l).or_else(|| OWN_LINE_ID.captures(l))
            && !ids.iter().any(|id| id == &m[1])
        {
            ids.push(m[1].to_string());
        }
    }
    ids
}

/// New lines with `id` attached to `block`, and the 0-based line the id sits on.
pub fn attach_block_id<S: AsRef<str>>(lines: &[S], block: Block, id: &str) -> Attached {
    let mut out: Vec<String> = lines.iter().map(|l| l.as_ref().to_string()).collect();
    if block.kind == BlockKind::Inline {
        let l = &out[block.end];
        let cr = if l.ends_with('\r') { "\r" } else { "" };
        out[block.end] = format!("{} ^{id}{cr}", clean(l));
        return Attached {
            lines: out,
            line: block.end,
        };
    }

    let eol = if out.iter().any(|l| l.ends_with('\r')) {
        "\r"
    } else {
        ""
    };
    let after = block.end + 1;
    let has_next = after < out.len();
    // The block may be the last line of a file with no final newline; it now
    // gets a successor, so it needs the line ending the others carry.
    if !eol.is_empty() && !out[block.end].ends_with('\r') {
        out[block.end].push_str(eol);
    }
    let mut insert = vec![
        eol.to_string(),
        format!("^{id}{}", if has_next { eol } else { "" }),
    ];
    if has_next && !is_blank(&out[after]) {
        insert.push(eol.to_string());
    }
    out.splice(after..after, insert);
    Attached {
        lines: out,
        line: after + 1,
    }
}

/// A 6-character lowercase id not in `taken`.
pub fn generate_block_id(taken: &[String]) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    loop {
        let mut bytes = [0u8; 6];
        getrandom::fill(&mut bytes).expect("the OS random source works");
        let id: String = bytes
            .iter()
            .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
            .collect();
        if !taken.contains(&id) {
            return id;
        }
    }
}

/// 0-based lines of body headings whose text is `heading` (leading `#`s optional).
pub fn find_heading_lines<S: AsRef<str>>(lines: &[S], heading: &str) -> Vec<usize> {
    let want = heading_text(heading);
    let body_start = frontmatter_end(lines).map_or(0, |e| e + 1);
    let fenced = fence_ranges(lines, body_start);
    (body_start..lines.len())
        .filter(|&i| {
            let l = clean(lines[i].as_ref());
            HEADING.is_match(l)
                && !fenced.iter().any(|(a, b)| i >= *a && i <= *b)
                && heading_text(l) == want
        })
        .collect()
}

fn heading_text(h: &str) -> String {
    let t = js::trim(h);
    let t = HEADING_PREFIX.replace(t, "");
    let t = HEADING_ID.replace(&t, "");
    let t = CLOSING_HASHES.replace(&t, "");
    js::trim(&t).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn own(start: usize, end: usize) -> Block {
        Block {
            kind: BlockKind::OwnLine,
            start,
            end,
        }
    }

    #[test]
    fn bounds_a_paragraph_by_blank_lines_and_other_blocks() {
        let lines = ["# H", "one", "two", "- item", "three"];
        assert_eq!(
            locate_block(&lines, 1).unwrap(),
            Block {
                kind: BlockKind::Inline,
                start: 1,
                end: 2
            }
        );
        assert_eq!(
            locate_block(&lines, 4).unwrap(),
            Block {
                kind: BlockKind::Inline,
                start: 4,
                end: 4
            }
        );
    }

    #[test]
    fn refuses_frontmatter_and_blank_lines_and_spans_fences_and_tables() {
        let lines = [
            "---", "a: 1", "---", "", "```", "x", "```", "| a |", "| b |",
        ];
        assert_eq!(
            locate_block(&lines, 1).unwrap_err().to_string(),
            "Line 2 is in the frontmatter; a block id needs a body line"
        );
        assert_eq!(
            locate_block(&lines, 3).unwrap_err().to_string(),
            "Line 4 is blank; a block id needs a line with content"
        );
        assert_eq!(locate_block(&lines, 5).unwrap(), own(4, 6));
        assert_eq!(locate_block(&lines, 8).unwrap(), own(7, 8));
    }

    #[test]
    fn reads_an_own_line_id_directly_after_the_block_or_after_one_blank_line() {
        let block = own(0, 0);
        assert_eq!(
            existing_block_id(&["> q", "^a"], block),
            Some(ExistingId {
                id: "a".into(),
                line: 1
            })
        );
        assert_eq!(
            existing_block_id(&["> q", "", "^b"], block),
            Some(ExistingId {
                id: "b".into(),
                line: 2
            })
        );
        assert_eq!(existing_block_id(&["> q", "", "text"], block), None);
        assert_eq!(existing_block_id(&["> q"], block), None);
    }

    #[test]
    fn does_not_add_a_blank_line_when_one_already_follows() {
        assert_eq!(
            attach_block_id(&["| a |", "", "next"], own(0, 0), "z").lines,
            ["| a |", "", "^z", "", "next"]
        );
    }

    #[test]
    fn attaches_inline_and_keeps_crlf() {
        let a = attach_block_id(
            &["para  \r", "x\r"],
            Block {
                kind: BlockKind::Inline,
                start: 0,
                end: 0,
            },
            "id",
        );
        assert_eq!(a.lines, ["para ^id\r", "x\r"]);
        let b = attach_block_id(&["x\r", "> q"], own(1, 1), "q");
        assert_eq!(
            (b.lines, b.line),
            (
                vec!["x\r".to_string(), "> q\r".into(), "\r".into(), "^q".into()],
                3
            )
        );
    }

    #[test]
    fn collects_trailing_and_own_line_ids_and_generates_fresh_ones() {
        let taken = all_block_ids(&["text ^one", "^two", "a^not"]);
        assert_eq!(taken, ["one", "two"]);
        let id = generate_block_id(&taken);
        assert!(!taken.contains(&id));
        assert!(BLOCK_ID.is_match(&id) && id.len() == 6);
    }

    #[test]
    fn matches_headings_outside_fences_ignoring_closing_hashes_and_ids() {
        let lines = [
            "## Plan ##",
            "```",
            "## Plan",
            "```",
            "### Plan ^p",
            "#Plan",
        ];
        assert_eq!(find_heading_lines(&lines, "Plan"), vec![0, 4]);
    }
}
