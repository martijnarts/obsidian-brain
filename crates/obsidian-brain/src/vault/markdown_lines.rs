//! Line-level structure of a markdown file: which lines are YAML frontmatter
//! and which sit inside fenced code. Callers split the raw file on `\n`, so a
//! CRLF file keeps a trailing `\r` on each line; every check here ignores it.

use std::sync::LazyLock;

use regex::Regex;

use crate::util::js;

static FENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[ \t]*(`{3,}|~{3,})").expect("valid pattern"));

pub fn strip_cr(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

/// 0-based index of the line that closes a leading frontmatter block, or
/// `None` when there is none. Frontmatter only counts when the first line is
/// `---` and a later `---` or `...` closes it.
pub fn frontmatter_end<S: AsRef<str>>(lines: &[S]) -> Option<usize> {
    let first = lines.first()?;
    if js::trim_end(strip_cr(first.as_ref())) != "---" {
        return None;
    }
    lines.iter().enumerate().skip(1).find_map(|(i, l)| {
        let l = js::trim_end(strip_cr(l.as_ref()));
        (l == "---" || l == "...").then_some(i)
    })
}

/// Inclusive 0-based `(open, close)` line ranges of fenced code blocks. A
/// fence closes on a line of the same character at least as long as the
/// opener; an unclosed fence runs to the end of the file.
pub fn fence_ranges<S: AsRef<str>>(lines: &[S], from: usize) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut open: Option<(usize, String)> = None;
    for (i, line) in lines.iter().enumerate().skip(from) {
        let Some(m) = FENCE.captures(strip_cr(line.as_ref())) else {
            continue;
        };
        let fence = &m[1];
        match &open {
            None => open = Some((i, fence.to_string())),
            Some((at, opener)) => {
                if fence.as_bytes()[0] == opener.as_bytes()[0] && fence.len() >= opener.len() {
                    ranges.push((*at, i));
                    open = None;
                }
            }
        }
    }
    if let Some((at, _)) = open {
        ranges.push((at, lines.len() - 1));
    }
    ranges
}

/// Per-line flag: true for frontmatter lines and lines inside a code fence.
pub fn non_body_lines<S: AsRef<str>>(lines: &[S]) -> Vec<bool> {
    let mut skip = vec![false; lines.len()];
    let body_start = match frontmatter_end(lines) {
        Some(end) => {
            skip[..=end].fill(true);
            end + 1
        }
        None => 0,
    };
    for (a, b) in fence_ranges(lines, body_start) {
        skip[a..=b].fill(true);
    }
    skip
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_closed_frontmatter_only() {
        assert_eq!(frontmatter_end(&["---", "a: 1", "...", "body"]), Some(2));
        assert_eq!(frontmatter_end(&["---\r", "a: 1\r", "---\r"]), Some(2));
        assert_eq!(frontmatter_end(&["---", "never closed"]), None);
        assert_eq!(frontmatter_end::<&str>(&[]), None);
    }

    #[test]
    fn pairs_fences_by_character_and_length_and_runs_an_unclosed_one_to_the_end() {
        assert_eq!(
            fence_ranges(&["````", "```", "````", "x", "~~~", "y"], 0),
            vec![(0, 2), (4, 5)]
        );
        assert_eq!(
            non_body_lines(&["---", "k: v", "---", "```", "code", "```", "text"]),
            vec![true, true, true, true, true, true, false]
        );
    }
}
