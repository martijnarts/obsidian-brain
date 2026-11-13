//! `find_notes_by_name`: a fuzzy quick-switcher over names, titles and aliases.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};

use super::{check_non_empty, check_range};
use crate::mcp::tool::ToolSpec;
use crate::types::Frontmatter;
use crate::util::js;
use crate::vault::fuzzy::similarity;
use crate::vault::vault_path::{in_folder, normalize_folder};

const DESCRIPTION: &str = "Find notes by a half-remembered name, like Obsidian's quick switcher: fuzzy, case-insensitive match against the filename, title and frontmatter `aliases`, best first. Use `grep_vault` or `search` to look inside note bodies instead.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Name fragment. Typos and non-contiguous characters still match.
    #[schemars(length(min = 1))]
    query: String,
    /// Only notes under this vault-relative folder.
    folder: Option<String>,
    /// Max results (1-100). Default 20.
    #[schemars(range(min = 1, max = 100))]
    limit: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchedOn {
    Name,
    Title,
    Alias,
}

impl MatchedOn {
    fn as_str(self) -> &'static str {
        match self {
            MatchedOn::Name => "name",
            MatchedOn::Title => "title",
            MatchedOn::Alias => "alias",
        }
    }
}

#[derive(Debug, Clone)]
struct Hit {
    path: String,
    title: String,
    matched_on: MatchedOn,
    alias: Option<String>,
    score: f64,
}

/// Score how well `query` names `candidate`, in [0, 1]. Tiers, best first:
/// exact, prefix, substring, every query word present, characters in order
/// (quick-switcher style), then a Levenshtein typo match. Both sides are
/// compared lower-cased. 0 means no match.
pub fn score_name(query: &str, candidate: &str) -> f64 {
    let q = js::trim(query).to_lowercase();
    let c = candidate.to_lowercase();
    if q.is_empty() || c.is_empty() {
        return 0.0;
    }
    if c == q {
        return 1.0;
    }
    if c.starts_with(&q) {
        return 0.9;
    }
    if let Some(idx) = c.find(&q) {
        let before = c[..idx].chars().next_back();
        let boundary = before.is_some_and(|ch| js::is_js_whitespace(ch) || "-_/.".contains(ch));
        return if boundary { 0.85 } else { 0.8 };
    }
    let words: Vec<&str> = q
        .split(js::is_js_whitespace)
        .filter(|w| !w.is_empty())
        .collect();
    if words.len() > 1 && words.iter().all(|w| c.contains(w)) {
        return 0.7;
    }

    let mut best: f64 = 0.0;
    // A tight span (characters close together) scores near 0.6, a loose one near 0.4.
    if let Some(span) = subsequence_span(&q, &c) {
        best = 0.4 + 0.2 * (js::utf16_len(&q) as f64 / span as f64);
    }
    let sim = similarity(&q, &c);
    if sim >= 0.6 {
        best = best.max(sim * 0.6);
    }
    best
}

/// Length, in UTF-16 units, of the shortest prefix-anchored window of `c`
/// holding `q`'s characters in order.
fn subsequence_span(q: &str, c: &str) -> Option<usize> {
    let hay: Vec<u16> = c.encode_utf16().collect();
    let mut start: Option<usize> = None;
    let mut pos = 0;
    let mut buf = [0u16; 2];
    for ch in q.chars() {
        if ch == ' ' {
            continue;
        }
        let needle: &[u16] = ch.encode_utf16(&mut buf);
        let found = (pos..=hay.len().saturating_sub(needle.len()))
            .find(|&i| hay.get(i..i + needle.len()) == Some(needle))?;
        start.get_or_insert(found);
        pos = found + 1;
    }
    start.map(|s| pos - s)
}

/// Frontmatter `aliases` (or legacy `alias`) as a list; a string is split on commas.
pub fn note_aliases(fm: &Frontmatter) -> Vec<String> {
    let mut out = Vec::new();
    for raw in [fm.get("aliases"), fm.get("alias")] {
        let list: Vec<Value> = match raw {
            Some(Value::Array(items)) => items.clone(),
            Some(Value::String(s)) => s.split(',').map(|p| Value::String(p.into())).collect(),
            _ => Vec::new(),
        };
        for a in list {
            if let Value::String(a) = a {
                let t = js::trim(&a);
                if !t.is_empty() {
                    out.push(t.to_string());
                }
            }
        }
    }
    out
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("find_notes_by_name", DESCRIPTION, |ctx, args| async move {
        check_non_empty("query", &args.query)?;
        check_range("limit", args.limit, 1, Some(100))?;
        ctx.db.run(move |c| find(c, args)).await
    })
}

fn find(c: &rusqlite::Connection, args: Args) -> anyhow::Result<Value> {
    let folder = normalize_folder(args.folder.as_deref())?;
    let limit = args.limit.unwrap_or(20) as usize;
    let mut stmt = c.prepare("SELECT id, title, frontmatter FROM nodes")?;
    let rows: Vec<(String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<Vec<(String, String, String)>>>()?
        .into_iter()
        .filter(|(id, _, _)| !id.starts_with("_stub/"))
        .collect();
    // A typo in the folder reads as an error, not as zero results.
    if !folder.is_empty() && !rows.iter().any(|(id, _, _)| in_folder(id, &folder)) {
        anyhow::bail!(
            "Folder not found: \"{}\"",
            args.folder.as_deref().unwrap_or("")
        );
    }

    let mut hits: Vec<Hit> = Vec::new();
    for (id, title, fm) in rows {
        if !in_folder(&id, &folder) {
            continue;
        }
        let fm: Frontmatter = serde_json::from_str(&fm)?;
        if fm.get("_stub") == Some(&Value::Bool(true)) {
            continue;
        }
        let last = id.rsplit('/').next().unwrap_or(&id);
        let stem = last.strip_suffix(".md").unwrap_or(last).to_string();

        // Strict `>` keeps the earlier field on a tie: name, then title, then alias.
        let mut best: Option<Hit> = None;
        let mut consider = |text: &str, matched_on: MatchedOn, alias: Option<&str>| {
            let score = score_name(&args.query, text);
            if score > 0.0 && best.as_ref().is_none_or(|b| score > b.score) {
                best = Some(Hit {
                    path: id.clone(),
                    title: title.clone(),
                    matched_on,
                    alias: alias.map(str::to_string),
                    score,
                });
            }
        };
        consider(&stem, MatchedOn::Name, None);
        consider(&title, MatchedOn::Title, None);
        for alias in note_aliases(&fm) {
            consider(&alias, MatchedOn::Alias, Some(&alias));
        }
        if let Some(b) = best {
            hits.push(b);
        }
    }

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| js::utf16_len(&a.path).cmp(&js::utf16_len(&b.path)))
            .then_with(|| js::locale_compare(&a.path, &b.path))
    });
    let total = hits.len();
    let results: Vec<Value> = hits
        .iter()
        .take(limit)
        .map(|h| {
            let mut row = Map::new();
            row.insert("path".into(), h.path.clone().into());
            row.insert("title".into(), h.title.clone().into());
            row.insert("matchedOn".into(), h.matched_on.as_str().into());
            // Math.round(score * 1000) / 1000
            let rounded = (h.score * 1000.0 + 0.5).floor() / 1000.0;
            row.insert("score".into(), rounded.into());
            if let Some(alias) = &h.alias {
                row.insert("alias".into(), alias.clone().into());
            }
            Value::Object(row)
        })
        .collect();
    let mut out = Map::new();
    out.insert("total".into(), total.into());
    if total > limit {
        out.insert("truncated".into(), true.into());
    }
    out.insert("results".into(), Value::Array(results));
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fm(v: Value) -> Frontmatter {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn ranks_exact_prefix_boundary_substring_words_subsequence() {
        let exact = score_name("kelly", "Kelly");
        let prefix = score_name("kel", "Kelly criterion");
        let boundary = score_name("crit", "Kelly criterion");
        let inner = score_name("elly", "Kelly criterion");
        let words = score_name("criterion kelly", "Kelly criterion");
        let subseq = score_name("klcr", "Kelly criterion");
        assert_eq!(exact, 1.0);
        assert!(prefix > boundary);
        assert!(boundary > inner);
        assert!(inner > words);
        assert!(words > subseq);
        assert!(subseq > 0.0);
    }

    #[test]
    fn tolerates_a_typo_through_levenshtein() {
        assert!(score_name("kubernets", "Kubernetes") > 0.4);
    }

    #[test]
    fn returns_zero_for_no_match_and_empty_input() {
        assert_eq!(score_name("zzz", "Kelly"), 0.0);
        assert_eq!(score_name("  ", "Kelly"), 0.0);
        assert_eq!(score_name("a", ""), 0.0);
    }

    #[test]
    fn subsequence_span_counts_utf16_units() {
        // `😀` is two units; the span runs from `a` to past `b`.
        assert_eq!(subsequence_span("ab", "a😀b"), Some(4));
        assert_eq!(subsequence_span("a b", "ab"), Some(2));
        assert_eq!(subsequence_span("ba", "ab"), None);
    }

    #[test]
    fn note_aliases_reads_lists_strings_and_the_legacy_key() {
        assert_eq!(
            note_aliases(&fm(json!({"aliases": ["A", " B ", "", 3]}))),
            ["A", "B"]
        );
        assert_eq!(
            note_aliases(&fm(json!({"aliases": "One, Two"}))),
            ["One", "Two"]
        );
        assert_eq!(note_aliases(&fm(json!({"alias": "Old"}))), ["Old"]);
        assert!(note_aliases(&fm(json!({}))).is_empty());
    }
}
