//! Fuzzy text matching based on Levenshtein distance.
//!
//! Ported from aaronsb/obsidian-mcp-plugin's `src/utils/fuzzy-match.ts`, but
//! adapted to work on raw strings instead of line-indexed content: it returns
//! offsets so the caller can substitute in place.
//!
//! Two stages:
//!   1. Direct substring search (case-insensitive).
//!   2. If no exact hit, sliding-window Levenshtein over word groups.
//!
//! Lengths and scores are computed over UTF-16 code units, as in JS, so the
//! scores match the TypeScript server. The offsets returned are byte
//! offsets into the haystack.

use crate::util::js::{self, is_js_whitespace};

#[derive(Debug, Clone, PartialEq)]
pub struct FuzzyMatch {
    /// Byte offset where the match begins (inclusive).
    pub start: usize,
    /// Byte offset where the match ends (exclusive).
    pub end: usize,
    /// The substring of the haystack that matched.
    pub text: String,
    /// Similarity score in [0, 1]; 1 means an exact (case-insensitive) match.
    pub score: f64,
}

/// Classic Levenshtein distance over UTF-16 code units.
fn levenshtein(a: &[u16], b: &[u16]) -> usize {
    let (m, n) = (a.len(), b.len());
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur = vec![0; m + 1];
    for i in 1..=n {
        cur[0] = i;
        for j in 1..=m {
            cur[j] = if b[i - 1] == a[j - 1] {
                prev[j - 1]
            } else {
                (prev[j - 1] + 1).min(cur[j - 1] + 1).min(prev[j] + 1)
            };
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m]
}

/// Similarity in [0,1]; 1 means identical (case-insensitive).
pub fn similarity(a: &str, b: &str) -> f64 {
    let max = js::utf16_len(a).max(js::utf16_len(b));
    if max == 0 {
        return 1.0;
    }
    let la: Vec<u16> = a.to_lowercase().encode_utf16().collect();
    let lb: Vec<u16> = b.to_lowercase().encode_utf16().collect();
    1.0 - levenshtein(&la, &lb) as f64 / max as f64
}

/// Locate fuzzy occurrences of `needle` inside `haystack`.
///
/// Returns all matches at or above `threshold` (the TS default is 0.7),
/// sorted by score descending. The caller enforces uniqueness (1 result is
/// good, more is ambiguous, 0 is missing).
pub fn fuzzy_find(haystack: &str, needle: &str, threshold: f64) -> Vec<FuzzyMatch> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut matches = Vec::new();

    // Stage 1: exact (case-insensitive) substring. Report every occurrence so
    // the caller can detect ambiguity. Offsets are UTF-16 indices into the
    // lowercased text, applied to the original, as the TS does.
    let hay16: Vec<u16> = haystack.encode_utf16().collect();
    let lower_hay: Vec<u16> = haystack.to_lowercase().encode_utf16().collect();
    let lower_needle: Vec<u16> = needle.to_lowercase().encode_utf16().collect();
    let needle_len = js::utf16_len(needle);
    let mut cursor = 0;
    while lower_needle.len() <= lower_hay.len() && cursor <= lower_hay.len() - lower_needle.len() {
        let Some(idx) = find_u16(&lower_hay, &lower_needle, cursor) else {
            break;
        };
        let end = (idx + needle_len).min(hay16.len());
        let start_b = js::utf16_to_byte(haystack, idx);
        let end_b = js::utf16_to_byte(haystack, end);
        matches.push(FuzzyMatch {
            start: start_b,
            end: end_b,
            text: haystack[start_b..end_b].to_string(),
            score: 1.0,
        });
        cursor = idx + needle_len;
    }
    if !matches.is_empty() {
        return matches;
    }

    // Stage 2: sliding window Levenshtein over word groups, line by line.
    let needle_words = js::trim(needle)
        .split(is_js_whitespace)
        .filter(|w| !w.is_empty())
        .count();
    let needle_len = needle_words.max(1);
    let mut line_start = 0;
    for line in haystack.split('\n') {
        let words = split_words(line);
        let max_window = words.len().min(needle_len + 2);
        let mut best: Option<(usize, usize, f64)> = None;
        for start in 0..words.len() {
            for end in start + 1..=words.len().min(start + max_window) {
                let phrase = words[start..end]
                    .iter()
                    .map(|w| w.1)
                    .collect::<Vec<_>>()
                    .join(" ");
                let score = similarity(&phrase, needle);
                if best.is_none_or(|b| score > b.2) {
                    let last = words[end - 1];
                    best = Some((words[start].0, last.0 + last.1.len(), score));
                }
            }
        }
        if let Some((s, e, score)) = best
            && score >= threshold
        {
            let (abs_start, abs_end) = (line_start + s, line_start + e);
            matches.push(FuzzyMatch {
                start: abs_start,
                end: abs_end,
                text: haystack[abs_start..abs_end].to_string(),
                score,
            });
        }
        line_start += line.len() + 1;
    }

    // A stable sort, as `Array.prototype.sort` is.
    matches.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    matches
}

/// Runs of non-whitespace (`/\S+/g`) with their byte offsets.
fn split_words(line: &str) -> Vec<(usize, &str)> {
    let mut words = Vec::new();
    let mut start = None;
    for (i, c) in line.char_indices() {
        match (is_js_whitespace(c), start) {
            (true, Some(s)) => {
                words.push((s, &line[s..i]));
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    if let Some(s) = start {
        words.push((s, &line[s..]));
    }
    words
}

fn find_u16(hay: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from);
    }
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_basics() {
        assert_eq!(similarity("Hello World", "hello world"), 1.0);
        let s = similarity("hello", "hallo");
        assert!(s > 0.0 && s < 1.0);
        assert_eq!(similarity("", ""), 1.0);
        assert!(
            (similarity("foo bar", "foo baz") - similarity("foo baz", "foo bar")).abs() < 1e-10
        );
    }

    #[test]
    fn finds_a_direct_case_insensitive_substring_match() {
        let hay = "The quick brown fox jumps over the lazy dog.";
        let hits = fuzzy_find(hay, "Quick Brown", 0.7);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].score, 1.0);
        assert_eq!(hits[0].text.to_lowercase(), "quick brown");
        assert_eq!(
            hay[hits[0].start..hits[0].end].to_lowercase(),
            "quick brown"
        );
    }

    #[test]
    fn reports_every_occurrence_of_an_exact_match() {
        let hits = fuzzy_find("foo foo foo", "foo", 0.7);
        assert_eq!(hits.len(), 3);
        assert!(hits.iter().all(|h| h.score == 1.0));
    }

    #[test]
    fn returns_empty_when_needle_is_empty() {
        assert!(fuzzy_find("anything", "", 0.7).is_empty());
    }

    #[test]
    fn finds_a_near_match_with_a_single_typo_above_threshold() {
        let hits = fuzzy_find(
            "The quick brown fox jumps over the lazy dog.",
            "quikc brown",
            0.7,
        );
        assert!(!hits.is_empty());
        assert!(hits[0].score >= 0.7 && hits[0].score < 1.0);
    }

    #[test]
    fn threshold_cutoff_and_zero_threshold() {
        assert!(
            fuzzy_find(
                "The quick brown fox jumps over the lazy dog.",
                "zzzzzz qqqqqq",
                0.9
            )
            .is_empty()
        );
        assert!(!fuzzy_find("Alpha beta gamma delta.", "zzz", 0.0).is_empty());
    }

    #[test]
    fn offsets_are_bytes_into_multibyte_text() {
        let hay = "über café\nnext line";
        let hits = fuzzy_find(hay, "CAFÉ", 0.7);
        assert_eq!(&hay[hits[0].start..hits[0].end], "café");
        let fuzzy = fuzzy_find(hay, "cafe", 0.7);
        assert_eq!(&hay[fuzzy[0].start..fuzzy[0].end], "café");
    }
}
