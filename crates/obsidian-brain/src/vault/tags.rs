//! Tag helpers over the indexed frontmatter JSON. A note's tags come from the
//! frontmatter `tags` / `tag` key (a list, or a comma- or space-separated
//! string, each entry with or without a leading `#`) plus the `inline_tags`
//! array the parser stores for body `#tags`. Tags compare case-insensitively,
//! as in Obsidian: `#Project` and `#project` are one tag.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_json::Value;

use crate::types::Frontmatter;
use crate::util::js::{is_js_whitespace, locale_compare};

fn split_tag_value(value: Option<&Value>, out: &mut Vec<String>) {
    match value {
        Some(Value::Array(items)) => items.iter().for_each(|v| split_tag_value(Some(v), out)),
        Some(Value::String(s)) => out.extend(
            s.split(|c: char| c == ',' || is_js_whitespace(c))
                .map(str::to_string),
        ),
        _ => {}
    }
}

/// Every distinct tag of one note, without the leading `#`. Of tags that
/// differ only in case, the first spelling is kept.
pub fn note_tags(frontmatter: &Frontmatter) -> Vec<String> {
    let mut raw = Vec::new();
    for key in ["tags", "tag", "inline_tags"] {
        split_tag_value(frontmatter.get(key), &mut raw);
    }
    let mut seen = HashSet::new();
    let mut tags = Vec::new();
    for t in raw {
        let tag = t.strip_prefix('#').unwrap_or(&t);
        if !tag.is_empty() && seen.insert(tag.to_lowercase()) {
            tags.push(tag.to_string());
        }
    }
    tags
}

/// True when `tag` is `filter` or nested below it (`a` matches `a/b`), ignoring case.
pub fn tag_matches(tag: &str, filter: &str) -> bool {
    let t = tag.to_lowercase();
    let f = filter.strip_prefix('#').unwrap_or(filter).to_lowercase();
    t == f || t.starts_with(&format!("{f}/"))
}

/// `a/b/c` -> `["a", "a/b", "a/b/c"]`.
pub fn tag_with_parents(tag: &str) -> Vec<String> {
    let parts: Vec<&str> = tag.split('/').collect();
    (1..=parts.len()).map(|i| parts[..i].join("/")).collect()
}

/// Note count per tag, case-insensitively; each tag is reported in the
/// spelling first seen. With `include_parents`, `a/b` also counts toward `a`.
/// The result keeps first-seen order, like the TS `Map`.
pub fn count_tags<'a>(
    frontmatters: impl IntoIterator<Item = &'a Frontmatter>,
    include_parents: bool,
) -> Vec<(String, usize)> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut slot: HashMap<String, usize> = HashMap::new();
    let mut spelling: HashMap<String, String> = HashMap::new();
    for fm in frontmatters {
        let mut tags: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        for t in note_tags(fm).iter().flat_map(|x| {
            if include_parents {
                tag_with_parents(x)
            } else {
                vec![x.clone()]
            }
        }) {
            let key = t.to_lowercase();
            spelling.entry(key.clone()).or_insert(t);
            if seen.insert(key.clone()) {
                tags.push(key);
            }
        }
        for key in tags {
            let name = &spelling[&key];
            match slot.get(name) {
                Some(&i) => counts[i].1 += 1,
                None => {
                    slot.insert(name.clone(), counts.len());
                    counts.push((name.clone(), 1));
                }
            }
        }
    }
    counts
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagCount {
    pub tag: String,
    pub count: usize,
}

/// Count desc, then name asc.
pub fn sort_tag_counts(counts: Vec<(String, usize)>) -> Vec<TagCount> {
    let mut out: Vec<TagCount> = counts
        .into_iter()
        .map(|(tag, count)| TagCount { tag, count })
        .collect();
    out.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| locale_compare(&a.tag, &b.tag))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fm(v: Value) -> Frontmatter {
        v.as_object().unwrap().clone()
    }

    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }

    #[test]
    fn note_tags_merges_tags_tag_and_inline_tags_strips_hash_and_dedupes() {
        assert_eq!(
            sorted(note_tags(&fm(
                json!({"tags": ["#a", "b/c"], "tag": "a", "inline_tags": ["d", "b/c"]})
            ))),
            ["a", "b/c", "d"]
        );
    }

    #[test]
    fn note_tags_splits_a_string_value_on_commas_and_whitespace() {
        assert_eq!(
            sorted(note_tags(&fm(json!({"tags": "#x, y z"})))),
            ["x", "y", "z"]
        );
    }

    #[test]
    fn note_tags_ignores_non_string_values_and_empty_entries() {
        assert!(note_tags(&fm(json!({"tags": [1, null, "", "#"]}))).is_empty());
        assert!(note_tags(&fm(json!({}))).is_empty());
    }

    #[test]
    fn tag_matches_accepts_the_tag_itself_and_nested_tags_only() {
        assert!(tag_matches("a", "a"));
        assert!(tag_matches("a/b", "a"));
        assert!(tag_matches("a/b", "#a"));
        assert!(!tag_matches("ab", "a"));
        assert!(!tag_matches("a", "a/b"));
    }

    #[test]
    fn tag_with_parents_expands_every_ancestor() {
        assert_eq!(tag_with_parents("a/b/c"), ["a", "a/b", "a/b/c"]);
        assert_eq!(tag_with_parents("a"), ["a"]);
    }

    #[test]
    fn count_tags_counts_each_note_once_per_tag() {
        let fms = [
            fm(json!({"tags": ["a/b", "a/c"]})),
            fm(json!({"tags": ["a"]})),
        ];
        let as_map = |v: Vec<(String, usize)>| v.into_iter().collect::<HashMap<_, _>>();
        assert_eq!(
            as_map(count_tags(&fms, true)),
            HashMap::from([("a".into(), 2), ("a/b".into(), 1), ("a/c".into(), 1)])
        );
        assert_eq!(
            as_map(count_tags(&fms, false)),
            HashMap::from([("a".into(), 1), ("a/b".into(), 1), ("a/c".into(), 1)])
        );
    }

    #[test]
    fn sort_tag_counts_orders_by_count_desc_then_name() {
        let counts = vec![("b".into(), 1), ("a".into(), 1), ("c".into(), 2)];
        let tags: Vec<_> = sort_tag_counts(counts).into_iter().map(|t| t.tag).collect();
        assert_eq!(tags, ["c", "a", "b"]);
    }

    #[test]
    fn treats_tags_that_differ_only_in_case_as_one_tag() {
        assert_eq!(
            note_tags(&fm(
                json!({"tags": ["Project", "project"], "inline_tags": ["PROJECT"]})
            )),
            ["Project"]
        );
        assert!(tag_matches("Project/Alpha", "project"));
        assert!(tag_matches("project", "#PROJECT"));
        let fms = [
            fm(json!({"tags": ["Project"]})),
            fm(json!({"tags": ["project/a"]})),
        ];
        assert_eq!(
            count_tags(&fms, true),
            vec![("Project".into(), 2), ("project/a".into(), 1)]
        );
    }
}
