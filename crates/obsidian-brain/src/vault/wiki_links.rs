//! Wiki-link extraction, resolution and rewriting.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::{Captures, Regex};

static FENCED_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"```[\s\S]*?```").expect("valid pattern"));
static INLINE_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`[^`]+`").expect("valid pattern"));
// Groups: 1=optional !, 2=stem, 3=optional # or ^ suffix, 4=optional | alias.
static REWRITE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(!?)\[\[([^\]|#^\n]+?)((?:#|\^)[^\]|\n]+)?(\|[^\]\n]*)?\]\]")
        .expect("valid pattern")
});

/// Filename stem to the paths that carry it.
pub type StemLookup = HashMap<String, Vec<String>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLink {
    /// The part to resolve (path or bare name).
    pub raw: String,
    /// Pipe alias display text, if any.
    pub display: Option<String>,
}

/// Markdown with fenced code blocks and inline code spans removed.
pub(crate) fn strip_code(markdown: &str) -> String {
    let without_fences = FENCED_CODE.replace_all(markdown, "");
    INLINE_CODE.replace_all(&without_fences, "").into_owned()
}

/// Extract wiki links from markdown, ignoring code blocks and embedded images.
pub fn extract_wiki_links(markdown: &str) -> Vec<RawLink> {
    let text = strip_code(markdown);
    let mut links = Vec::new();
    // `(?<!!)\[\[([^\]]+)\]\]`, scanned by hand to keep the lookbehind.
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(off) = text[i..].find("[[") {
        let at = i + off;
        if at > 0 && bytes[at - 1] == b'!' {
            i = at + 1;
            continue;
        }
        let inner_start = at + 2;
        match text[inner_start..].find(']') {
            Some(len) if len > 0 && text[inner_start + len..].starts_with("]]") => {
                let inner = &text[inner_start..inner_start + len];
                links.push(match inner.split_once('|') {
                    Some((raw, display)) => RawLink {
                        raw: raw.to_string(),
                        display: Some(display.to_string()),
                    },
                    None => RawLink {
                        raw: inner.to_string(),
                        display: None,
                    },
                });
                i = inner_start + len + 2;
            }
            _ => i = at + 1,
        }
    }
    links
}

fn stem_of(path: &str) -> &str {
    let no_ext = path.strip_suffix(".md").unwrap_or(path);
    no_ext.rsplit('/').next().unwrap_or(no_ext)
}

/// Build a lookup table mapping filename stems to their full paths.
pub fn build_stem_lookup<S: AsRef<str>>(all_paths: &[S]) -> StemLookup {
    let mut lookup = StemLookup::new();
    for p in all_paths {
        let p = p.as_ref();
        lookup
            .entry(stem_of(p).to_string())
            .or_default()
            .push(p.to_string());
    }
    lookup
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewrite {
    pub text: String,
    pub occurrences: usize,
}

/// Rewrite every wiki-link targeting `old_path` (as either a bare stem or a
/// path-qualified reference) to point at `new_path`.
///
/// `old_path` and `new_path` are vault-relative `.md` paths, such as
/// `"notes/BMW.md"` and `"cars/BMW & Audi.md"`. Three match shapes:
///
///   1. Bare stem `[[BMW]]` becomes `[[BMW & Audi]]`: the author's
///      shortest-path style stays, with the new basename.
///   2. Path-qualified `[[notes/BMW]]` becomes `[[cars/BMW & Audi]]`, so a
///      cross-folder rename does not leave the link in a defunct folder.
///   3. Path-qualified with `.md`, `[[notes/BMW.md]]`, is normalised to the
///      form of (2).
///
/// The decorations `[[x|alias]]`, `![[x]]`, `[[x#heading]]` and
/// `[[x^block]]` survive in all cases.
pub fn rewrite_wiki_links(markdown: &str, old_path: &str, new_path: &str) -> Rewrite {
    let old_stem = stem_of(old_path);
    let old_path_no_ext = old_path.strip_suffix(".md").unwrap_or(old_path);
    let new_stem = stem_of(new_path);
    let new_path_no_ext = new_path.strip_suffix(".md").unwrap_or(new_path);

    let mut occurrences = 0;
    let text = REWRITE.replace_all(markdown, |c: &Captures| {
        let trimmed = crate::util::js::trim(&c[2]);
        let mut replacement = None;
        if trimmed == old_stem {
            // A cross-folder-only move leaves bare-stem references resolving
            // on their own, so only a changed stem is rewritten.
            if old_stem != new_stem {
                replacement = Some(new_stem);
            }
        } else if (trimmed == old_path_no_ext || trimmed == old_path)
            && old_path_no_ext != new_path_no_ext
        {
            replacement = Some(new_path_no_ext);
        }
        match replacement {
            None => c[0].to_string(),
            Some(r) => {
                occurrences += 1;
                let get = |i| c.get(i).map_or("", |m| m.as_str());
                format!("{}[[{}{}{}]]", get(1), r, get(3), get(4))
            }
        }
    });
    Rewrite {
        text: text.into_owned(),
        occurrences,
    }
}

/// Resolve a wiki link reference to a vault-relative path. Returns `None`
/// for unresolvable links (these become stub nodes).
///
/// `referrer_path`: when given, a candidate in the referrer's folder wins
/// over other candidates, as in Obsidian's own resolver.
///
/// `warned_ambiguous`: a set the caller threads through one `parse_vault`,
/// so an ambiguous stem is warned about once per pass instead of once per
/// occurrence.
pub fn resolve_link(
    raw: &str,
    stem_lookup: &StemLookup,
    all_paths: Option<&HashSet<String>>,
    referrer_path: Option<&str>,
    mut warned_ambiguous: Option<&mut HashSet<String>>,
) -> Option<String> {
    // Path-qualified: try a direct match first.
    let with_md = if raw.ends_with(".md") {
        raw.to_string()
    } else {
        format!("{raw}.md")
    };
    let direct = match all_paths {
        Some(set) => set.contains(&with_md),
        None => stem_lookup.values().flatten().any(|p| *p == with_md),
    };
    if direct {
        return Some(with_md);
    }

    // Bare name: look up the stem.
    let stem = raw.rsplit('/').next().unwrap_or(raw);
    let candidates = stem_lookup.get(stem).filter(|c| !c.is_empty())?;
    if candidates.len() == 1 {
        return Some(candidates[0].clone());
    }

    // Ambiguous: prefer a path-qualified match if the raw includes a directory.
    if raw.contains('/') {
        let prefix = raw.strip_suffix(".md").unwrap_or(raw);
        if let Some(m) = candidates.iter().find(|c| c.starts_with(prefix)) {
            return Some(m.clone());
        }
    }

    if let Some(referrer) = referrer_path {
        let dir = |p: &str| {
            p.rsplit_once('/')
                .map_or(String::new(), |(d, _)| d.to_string())
        };
        let ref_dir = dir(referrer);
        if let Some(same) = candidates.iter().find(|c| dir(c) == ref_dir) {
            return Some(same.clone());
        }
    }

    let warned = warned_ambiguous.as_ref().is_some_and(|w| w.contains(raw));
    if !warned {
        tracing::warn!(
            link = raw,
            candidates = ?candidates,
            "Ambiguous wiki link [[{raw}]]: {}. Using first match.",
            candidates.join(", ")
        );
        if let Some(w) = warned_ambiguous.as_mut() {
            w.insert(raw.to_string());
        }
    }
    Some(candidates[0].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(raw: &str, display: Option<&str>) -> RawLink {
        RawLink {
            raw: raw.into(),
            display: display.map(Into::into),
        }
    }

    #[test]
    fn extracts_bare_path_and_aliased_links() {
        assert_eq!(
            extract_wiki_links("See [[Alice Smith]] for details."),
            vec![link("Alice Smith", None)]
        );
        assert_eq!(
            extract_wiki_links("Uses [[Concepts/Widget Theory]] extensively."),
            vec![link("Concepts/Widget Theory", None)]
        );
        assert_eq!(
            extract_wiki_links("The [[Concepts/Widget Theory|widget framework]] works."),
            vec![link("Concepts/Widget Theory", Some("widget framework"))]
        );
    }

    #[test]
    fn ignores_code_and_embeds() {
        assert_eq!(
            extract_wiki_links("```\n[[not a link]]\n```\nBut [[real link]] is."),
            vec![link("real link", None)]
        );
        assert_eq!(
            extract_wiki_links("Look at ![[photo.png]] and [[real link]]."),
            vec![link("real link", None)]
        );
        assert_eq!(
            extract_wiki_links("Code `[[x]]` and [[y]]"),
            vec![link("y", None)]
        );
        // The lookbehind fails at the first `[[`; the scan resumes inside it.
        assert_eq!(extract_wiki_links("![[[x]]"), vec![link("x", None)]);
    }

    #[test]
    fn extracts_multiple_links_from_one_paragraph() {
        assert_eq!(
            extract_wiki_links("Both [[Alice]] and [[Bob]] agreed on [[Plan]].").len(),
            3
        );
    }

    #[test]
    fn build_stem_lookup_maps_stems_and_detects_ambiguity() {
        let lookup = build_stem_lookup(&["People/Alice Smith.md", "Concepts/Widget Theory.md"]);
        assert_eq!(lookup["Alice Smith"], ["People/Alice Smith.md"]);
        assert_eq!(lookup["Widget Theory"], ["Concepts/Widget Theory.md"]);
        assert_eq!(
            build_stem_lookup(&["People/Alice Smith.md", "Archive/Alice Smith.md"])["Alice Smith"]
                .len(),
            2
        );
    }

    #[test]
    fn resolve_link_basics() {
        let lookup = build_stem_lookup(&[
            "People/Alice Smith.md",
            "People/Bob Jones.md",
            "Concepts/Widget Theory.md",
        ]);
        assert_eq!(
            resolve_link("Alice Smith", &lookup, None, None, None).as_deref(),
            Some("People/Alice Smith.md")
        );
        assert_eq!(
            resolve_link("People/Bob Jones", &lookup, None, None, None).as_deref(),
            Some("People/Bob Jones.md")
        );
        assert_eq!(
            resolve_link("Nonexistent Page", &lookup, None, None, None),
            None
        );
    }

    #[test]
    fn ambiguous_warning_dedups_through_the_set() {
        let lookup = build_stem_lookup(&[
            "Fleeting/America.md",
            "Misc/America.md",
            "Permanent/America.md",
        ]);
        let mut warned = HashSet::new();
        for _ in 0..5 {
            resolve_link(
                "America",
                &lookup,
                None,
                Some("Daily/2024-01-01.md"),
                Some(&mut warned),
            );
        }
        assert!(warned.contains("America"));
        assert_eq!(warned.len(), 1);
    }

    #[test]
    fn prefers_same_folder_candidates() {
        let lookup = build_stem_lookup(&["A/target.md", "B/target.md", "C/target.md"]);
        assert_eq!(
            resolve_link("target", &lookup, None, Some("A/note.md"), None).as_deref(),
            Some("A/target.md")
        );
        assert_eq!(
            resolve_link("target", &lookup, None, Some("B/note.md"), None).as_deref(),
            Some("B/target.md")
        );
        assert_eq!(
            resolve_link("target", &lookup, None, Some("X/note.md"), None).as_deref(),
            Some("A/target.md")
        );
        assert_eq!(
            resolve_link("target", &lookup, None, None, None).as_deref(),
            Some("A/target.md")
        );
    }

    fn rw(input: &str, old: &str, new: &str) -> (String, usize) {
        let r = rewrite_wiki_links(input, old, new);
        (r.text, r.occurrences)
    }

    #[test]
    fn rewrite_keeps_decorations() {
        assert_eq!(
            rw("See [[old]] for details.", "old", "new"),
            ("See [[new]] for details.".into(), 1)
        );
        assert_eq!(
            rw("The [[old|widget framework]] works.", "old", "new"),
            ("The [[new|widget framework]] works.".into(), 1)
        );
        assert_eq!(
            rw("Embed: ![[old]] end.", "old", "new"),
            ("Embed: ![[new]] end.".into(), 1)
        );
        assert_eq!(
            rw("Jump to [[old#Intro]].", "old", "new"),
            ("Jump to [[new#Intro]].".into(), 1)
        );
        assert_eq!(
            rw("Block ref [[old^abc123]] here.", "old", "new"),
            ("Block ref [[new^abc123]] here.".into(), 1)
        );
    }

    #[test]
    fn rewrite_leaves_other_links_and_counts() {
        assert_eq!(
            rw("Mentions [[other]] and [[another]].", "old", "new"),
            ("Mentions [[other]] and [[another]].".into(), 0)
        );
        assert_eq!(
            rw(
                "First [[old]], then ![[old]], and [[old|alias]] plus [[other]].",
                "old",
                "new"
            ),
            (
                "First [[new]], then ![[new]], and [[new|alias]] plus [[other]].".into(),
                3
            )
        );
        assert_eq!(rw("", "old", "new"), (String::new(), 0));
        assert_eq!(
            rw("Mention [[old-notes]] here.", "old", "new"),
            ("Mention [[old-notes]] here.".into(), 0)
        );
        assert_eq!(
            rw("Padded [[ old ]] link.", "old", "new"),
            ("Padded [[new]] link.".into(), 1)
        );
    }

    #[test]
    fn rewrite_treats_special_characters_literally() {
        assert_eq!(
            rw("I drive a [[BMW]] today.", "BMW", "BMW & Audi"),
            ("I drive a [[BMW & Audi]] today.".into(), 1)
        );
        assert_eq!(
            rw("See [[Notes]] here.", "Notes", "Notes (2025)"),
            ("See [[Notes (2025)]] here.".into(), 1)
        );
        assert_eq!(
            rw("Template in [[C++]] is hard.", "C++", "C Plus Plus"),
            ("Template in [[C Plus Plus]] is hard.".into(), 1)
        );
        assert_eq!(
            rw("Price in [[dollars]].", "dollars", "$amount"),
            ("Price in [[$amount]].".into(), 1)
        );
    }

    #[test]
    fn rewrite_path_qualified_links() {
        let (o, n) = ("notes/BMW.md", "cars/BMW & Audi.md");
        assert_eq!(
            rw("See [[notes/BMW]] for details.", o, n),
            ("See [[cars/BMW & Audi]] for details.".into(), 1)
        );
        assert_eq!(
            rw("See [[BMW]] for details.", o, n),
            ("See [[BMW & Audi]] for details.".into(), 1)
        );
        assert_eq!(
            rw("See [[notes/BMW.md]] for details.", o, n),
            ("See [[cars/BMW & Audi]] for details.".into(), 1)
        );
        assert_eq!(
            rw("Jump to [[notes/BMW#Specs]] now.", o, n),
            ("Jump to [[cars/BMW & Audi#Specs]] now.".into(), 1)
        );
        assert_eq!(
            rw("Other bike: [[other/BMW]] — unrelated.", o, n),
            ("Other bike: [[other/BMW]] — unrelated.".into(), 0)
        );
        assert_eq!(
            rw("Active [[notes/BMW]] and archived [[archive/BMW]].", o, n),
            (
                "Active [[cars/BMW & Audi]] and archived [[archive/BMW]].".into(),
                1
            )
        );
    }
}
