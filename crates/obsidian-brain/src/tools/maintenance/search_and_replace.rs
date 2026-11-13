//! `search_and_replace`: vault-wide find/replace in note bodies. Dry run by
//! default. Frontmatter and fenced code blocks are left alone unless the
//! caller opts in. Every file is planned before any is written, so the
//! `max_files` cap refuses a too-wide replace without touching the vault.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{blocking, require_non_empty, write_note};
use crate::context::VaultContext;
use crate::mcp::tool::ToolSpec;
use crate::tools::support::background_reindex::run_background_reindex;
use crate::util::errors::error_message;
use crate::util::js::{byte_to_utf16, utf16_to_byte};
use crate::util::safe_regex::{UserRegex, compile_user_regex};
use crate::vault::scan::{fenced_code_ranges, line_at, list_note_paths, split_frontmatter};
use crate::vault::vault_path::{read_note_file, resolve_vault_path};

const DESCRIPTION: &str = "Find and replace text across note bodies. Dry run by default: returns per-file match counts and before/after samples; pass `dryRun: false` to write. Skips frontmatter and fenced code unless asked.";

const SAMPLES_PER_FILE: usize = 3;
/// Context around a sample, in UTF-16 code units.
const SAMPLE_CONTEXT: usize = 80;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Text to find, or a JavaScript regex when `regex` is true. `^`/`$` match at line ends.
    #[schemars(length(min = 1))]
    pattern: String,
    /// Replacement text. With `regex`, `$1`, `$<name>` and `$&` expand.
    replacement: String,
    /// Treat `pattern` as a regex. Default false.
    regex: Option<bool>,
    /// Default true.
    case_sensitive: Option<bool>,
    /// Only touch notes under this folder.
    folder: Option<String>,
    /// Also replace inside YAML frontmatter. Default false.
    include_frontmatter: Option<bool>,
    /// Also replace inside fenced code blocks. Default false.
    include_code: Option<bool>,
    /// Default true. Pass false to write.
    dry_run: Option<bool>,
    /// Refuse to write when more files match. Default 200.
    #[schemars(range(min = 1))]
    max_files: Option<u64>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("search_and_replace", DESCRIPTION, |ctx, args| async move {
        require_non_empty("pattern", &args.pattern)?;
        blocking(ctx, move |ctx| run(ctx, args)).await
    })
}

/// One match: `[start, end)` bytes in the raw file and the text that
/// replaces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Scope {
    pub include_frontmatter: bool,
    pub include_code: bool,
}

struct Planned {
    path: String,
    raw: String,
    hits: Vec<Hit>,
}

fn run(ctx: &Arc<VaultContext>, args: Args) -> Result<Value> {
    let dry_run = args.dry_run != Some(false);
    let max_files = args
        .max_files
        .map_or(200, |n| usize::try_from(n).unwrap_or(usize::MAX));
    let flags = if args.case_sensitive == Some(false) {
        "gmi"
    } else {
        "gm"
    };
    let expand = args.regex == Some(true);
    let re = compile_user_regex(&args.pattern, flags, !expand)?;
    let vault = &ctx.config.vault_path;
    let scope = Scope {
        include_frontmatter: args.include_frontmatter == Some(true),
        include_code: args.include_code == Some(true),
    };

    let paths = ctx
        .db
        .with(|conn| list_note_paths(conn, args.folder.as_deref(), &[]))?;
    let mut planned = Vec::new();
    for path in paths {
        let Ok(note) = read_note_file(vault, &path) else {
            continue;
        };
        let hits = find_hits(&note.content, &re, &args.replacement, expand, scope)?;
        if !hits.is_empty() {
            planned.push(Planned {
                path,
                raw: note.content,
                hits,
            });
        }
    }

    let files_matched = planned.len();
    let total_matches: usize = planned.iter().map(|f| f.hits.len()).sum();
    if !dry_run && files_matched > max_files {
        bail!(
            "{files_matched} files match, more than maxFiles ({max_files}). Nothing was written. Narrow `folder` or raise `maxFiles`."
        );
    }

    if dry_run {
        let mut out = Map::new();
        out.insert("dryRun".into(), true.into());
        out.insert("filesMatched".into(), files_matched.into());
        out.insert("totalMatches".into(), total_matches.into());
        if files_matched > max_files {
            out.insert("truncated".into(), true.into());
        }
        let files: Vec<Value> = planned
            .iter()
            .take(max_files)
            .map(|f| {
                json!({
                    "path": f.path,
                    "matches": f.hits.len(),
                    "samples": samples_of(&f.raw, &f.hits),
                })
            })
            .collect();
        out.insert("files".into(), Value::Array(files));
        return Ok(Value::Object(out));
    }

    let mut failed = Vec::new();
    let mut files = Vec::new();
    let mut written_matches = 0;
    for f in &planned {
        let result = resolve_vault_path(vault, &f.path)
            .map_err(|e| error_message(&e))
            .and_then(|p| write_note(&p.abs, &apply_hits(&f.raw, &f.hits)));
        match result {
            Ok(()) => {
                written_matches += f.hits.len();
                files.push(json!({"path": f.path, "matches": f.hits.len()}));
            }
            Err(error) => failed.push(json!({"path": f.path, "error": error})),
        }
    }

    // Fire-and-forget reindex, once for the whole batch.
    if !files.is_empty() {
        run_background_reindex(ctx);
    }

    let mut out = Map::new();
    out.insert("dryRun".into(), false.into());
    out.insert("filesChanged".into(), files.len().into());
    out.insert("totalMatches".into(), written_matches.into());
    out.insert("files".into(), Value::Array(files));
    if !failed.is_empty() {
        out.insert("failed".into(), Value::Array(failed));
    }
    Ok(Value::Object(out))
}

/// Every match in the editable parts of `raw` with its replacement text.
/// Each editable segment is matched on its own, so a match never spans
/// frontmatter or a code fence.
pub fn find_hits(
    raw: &str,
    re: &UserRegex,
    replacement: &str,
    expand: bool,
    scope: Scope,
) -> Result<Vec<Hit>> {
    let region_start = if scope.include_frontmatter {
        0
    } else {
        split_frontmatter(raw).frontmatter.len()
    };
    let region = &raw[region_start..];
    let skip = if scope.include_code {
        Vec::new()
    } else {
        fenced_code_ranges(region)
    };

    let mut segments = Vec::new();
    let mut at = 0;
    for (s, e) in skip {
        if s > at {
            segments.push((at, s));
        }
        at = e;
    }
    if at < region.len() {
        segments.push((at, region.len()));
    }

    let named = re.regex.capture_names().any(|n| n.is_some());
    let mut hits = Vec::new();
    for (s, e) in segments {
        let segment = &region[s..e];
        for_each_match(re, segment, |caps| {
            let m = caps.get(0).expect("group 0");
            let start = region_start + s + m.start();
            hits.push(Hit {
                start,
                end: start + m.as_str().len(),
                text: if expand {
                    expand_template(replacement, caps, named, segment)
                } else {
                    replacement.to_string()
                },
            });
        })?;
    }
    Ok(hits)
}

/// `text.matchAll(re)` for a global regex: after an empty match the search
/// moves on one character.
fn for_each_match(
    re: &UserRegex,
    text: &str,
    mut f: impl FnMut(&fancy_regex::Captures<'_, str>),
) -> Result<()> {
    let mut pos = 0;
    while pos <= text.len() {
        let caps = re
            .regex
            .captures_from_pos(text, pos)
            .map_err(|e| anyhow!("Regex match failed: {e}"))?;
        let Some(caps) = caps else {
            break;
        };
        let m = caps.get(0).expect("group 0");
        f(&caps);
        pos = if m.end() > m.start() {
            m.end()
        } else {
            match text[m.end()..].chars().next() {
                Some(c) => m.end() + c.len_utf8(),
                None => break,
            }
        };
    }
    Ok(())
}

/// `String.prototype.replace` substitution rules for one match. `named` is
/// true when the regex has a named group, which is when JS gives the match
/// a `groups` object.
pub fn expand_template(
    template: &str,
    caps: &fancy_regex::Captures<'_, str>,
    named: bool,
    input: &str,
) -> String {
    let m = caps.get(0).expect("group 0");
    let groups = caps.len() - 1;
    let group = |n: usize| caps.get(n).map_or("", |g| g.as_str());
    let bytes = template.as_bytes();
    let mut out = String::with_capacity(template.len());
    let mut i = 0;
    // Copy runs of plain text whole, so multi-byte characters stay intact.
    let mut plain = 0;
    while i < bytes.len() {
        if bytes[i] != b'$' || i + 1 >= bytes.len() {
            i += 1;
            continue;
        }
        let token_start = i;
        let (text, next): (std::borrow::Cow<'_, str>, usize) = match bytes[i + 1] {
            b'$' => ("$".into(), i + 2),
            b'&' => (m.as_str().into(), i + 2),
            b'`' => (input[..m.start()].into(), i + 2),
            b'\'' => (input[m.end()..].into(), i + 2),
            b'<' => match template[i + 2..].find('>') {
                Some(rel) => {
                    let close = i + 2 + rel;
                    let name = &template[i + 2..close];
                    let text = if named {
                        caps.name(name).map_or("", |g| g.as_str()).into()
                    } else {
                        template[i..=close].into()
                    };
                    (text, close + 1)
                }
                None => {
                    i += 1;
                    continue;
                }
            },
            d1 @ b'0'..=b'9' => {
                let d2 = bytes.get(i + 2).copied().filter(u8::is_ascii_digit);
                let len = if d2.is_some() { 2 } else { 1 };
                let first = usize::from(d1 - b'0');
                let n = d2.map_or(first, |d| first * 10 + usize::from(d - b'0'));
                let text = if (1..=groups).contains(&n) {
                    group(n).into()
                } else if let Some(d) = d2.filter(|_| (1..=groups).contains(&first)) {
                    // `$12` with fewer than 12 groups reads as `$1` followed by `2`.
                    format!("{}{}", group(first), d as char).into()
                } else {
                    template[i..i + 1 + len].into()
                };
                (text, i + 1 + len)
            }
            _ => {
                i += 1;
                continue;
            }
        };
        out.push_str(&template[plain..token_start]);
        out.push_str(&text);
        i = next;
        plain = next;
    }
    out.push_str(&template[plain..]);
    out
}

fn apply_hits(raw: &str, hits: &[Hit]) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut at = 0;
    for h in hits {
        out.push_str(&raw[at..h.start]);
        out.push_str(&h.text);
        at = h.end;
    }
    out.push_str(&raw[at..]);
    out
}

/// Before/after excerpts for the first few lines that hold a match.
fn samples_of(raw: &str, hits: &[Hit]) -> Vec<Value> {
    let mut by_line: Vec<(usize, Vec<&Hit>)> = Vec::new();
    for h in hits {
        let line = line_at(raw, h.start);
        match by_line.iter().position(|(l, _)| *l == line) {
            Some(i) => by_line[i].1.push(h),
            None if by_line.len() >= SAMPLES_PER_FILE => break,
            None => by_line.push((line, vec![h])),
        }
    }
    by_line
        .into_iter()
        .map(|(line, group)| {
            let first = group[0];
            let last = group[group.len() - 1];
            let line_start = raw[..first.start].rfind('\n').map_or(0, |i| i + 1);
            let line_end = raw[last.end..]
                .find('\n')
                .map_or(raw.len(), |i| last.end + i);
            let back = byte_to_utf16(raw, first.start).saturating_sub(SAMPLE_CONTEXT);
            let from = line_start.max(utf16_to_byte(raw, back));
            let ahead = byte_to_utf16(raw, last.end) + SAMPLE_CONTEXT;
            let to = line_end.min(utf16_to_byte(raw, ahead));
            let shifted: Vec<Hit> = group
                .iter()
                .map(|h| Hit {
                    start: h.start - from,
                    end: h.end - from,
                    text: h.text.clone(),
                })
                .collect();
            let before = &raw[from..to];
            json!({"line": line, "before": before, "after": apply_hits(before, &shifted)})
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::test_support::{Chmod, Fixture};
    use super::*;

    /// `input.replace(new RegExp(pattern, flags), template)`, built from
    /// [`for_each_match`] and [`expand_template`].
    fn js_replace(pattern: &str, flags: &str, input: &str, template: &str) -> String {
        let re = compile_user_regex(pattern, flags, false).unwrap();
        let named = re.regex.capture_names().any(|n| n.is_some());
        let mut hits = Vec::new();
        let mut done = false;
        for_each_match(&re, input, |caps| {
            if done {
                return;
            }
            let m = caps.get(0).unwrap();
            hits.push(Hit {
                start: m.start(),
                end: m.end(),
                text: expand_template(template, caps, named, input),
            });
            done = !re.flags.global;
        })
        .unwrap();
        apply_hits(input, &hits)
    }

    #[test]
    fn expand_template_matches_js_string_replace() {
        // Expected outputs captured from Node's `String.prototype.replace`.
        let cases: &[(&str, &str, &str, &str, &str)] = &[
            (
                "(a)(b)?",
                "",
                "xaby",
                "$`|$'|$3|$12|$0|$<n>",
                "xx|y|$3|a2|$0|$<n>y",
            ),
            ("(?<n>a)", "", "a", "$<n>$<zz>", "a"),
            (
                "(f)oo-(?<n>\\d)",
                "gi",
                "Foo-1 foo-2",
                "$2|$<n>|$1|$&|$$",
                "1|1|F|Foo-1|$ 2|2|f|foo-2|$",
            ),
            (
                "(a)(b)(c)(d)(e)(f)(g)(h)(i)(j)(k)(l)",
                "",
                "abcdefghijkl",
                "$12|$10|$1|$13|$01|$00|$099|$1a",
                "l|j|a|a3|a|$00|i9|aa",
            ),
            (
                "(x)",
                "",
                "x",
                "$10|$1$|$$$|$|$<n>|$9|$x|$",
                "x0|x$|$$|$|$<n>|$9|$x|$",
            ),
            (
                "(?<a>x)(y)?",
                "",
                "xz",
                "[$<a>][$<b>][$<][$<a][$2][$<>]",
                "[x][][]z",
            ),
            ("b", "g", "abcb", "[$`|$']", "a[a|cb]c[abc|]"),
            ("x*", "g", "abc", "-$&-", "--a--b--c--"),
            (
                "é(ü)",
                "g",
                "aéüb🎉éü",
                "<$1|$`|$'>",
                "a<ü|a|b🎉éü>b🎉<ü|aéüb🎉|>",
            ),
            ("(a)|(b)", "g", "ab", "[$1,$2]", "[a,][,b]"),
            ("(a)(b)(c)", "g", "abc", "$4$34$31$3$03$30", "$4c4c1ccc0"),
            ("x*", "g", "xax", "[$&]", "[x][]a[x][]"),
            ("a", "g", "aa", "é$&ü", "éaüéaü"),
        ];
        for (pattern, flags, input, template, want) in cases {
            assert_eq!(
                js_replace(pattern, flags, input, template),
                *want,
                "/{pattern}/{flags} on {input:?} with {template:?}"
            );
        }
    }

    async fn setup(files: &[(&str, &str)]) -> Fixture {
        Fixture::new(files).await
    }

    #[tokio::test]
    async fn dry_run_is_the_default_counts_and_samples_no_writes_no_reindex() {
        let fx = setup(&[
            ("A.md", "cat and cat\nno match\nthe cat\n"),
            ("B.md", "dog"),
        ])
        .await;
        let out = fx
            .ok(&spec(), json!({"pattern": "cat", "replacement": "dog"}))
            .await;
        assert_eq!(
            out,
            json!({
                "dryRun": true,
                "filesMatched": 1,
                "totalMatches": 3,
                "files": [{
                    "path": "A.md",
                    "matches": 3,
                    "samples": [
                        {"line": 1, "before": "cat and cat", "after": "dog and dog"},
                        {"line": 3, "before": "the cat", "after": "the dog"},
                    ],
                }],
            })
        );
        assert_eq!(fx.read("A.md"), "cat and cat\nno match\nthe cat\n");
        assert_eq!(fx.reindex_calls(), 0);
    }

    #[tokio::test]
    async fn writes_with_dry_run_false_and_reindexes_once() {
        let fx = setup(&[("A.md", "a.b a.b"), ("B.md", "xa.by"), ("C.md", "none")]).await;
        let out = fx
            .ok(
                &spec(),
                json!({"pattern": "a.b", "replacement": "$1", "dryRun": false}),
            )
            .await;
        assert_eq!(out["dryRun"], false);
        assert_eq!(out["filesChanged"], 2);
        assert_eq!(out["totalMatches"], 3);
        assert_eq!(fx.read("A.md"), "$1 $1");
        assert_eq!(fx.read("B.md"), "x$1y");
        assert_eq!(fx.reindex_calls(), 1);
    }

    #[tokio::test]
    async fn regex_mode_expands_groups_case_sensitive_false_ignores_case() {
        let fx = setup(&[("A.md", "Foo-1 foo-2\n")]).await;
        fx.ok(
            &spec(),
            json!({
                "pattern": "(f)oo-(?<n>\\d)",
                "replacement": "$2|$<n>|$1|$&|$$",
                "regex": true,
                "caseSensitive": false,
                "dryRun": false,
            }),
        )
        .await;
        assert_eq!(fx.read("A.md"), "1|1|F|Foo-1|$ 2|2|f|foo-2|$\n");
    }

    #[tokio::test]
    async fn is_case_sensitive_by_default_and_anchors_caret_per_line() {
        let fx = setup(&[("A.md", "Cat\ncat\n")]).await;
        fx.ok(
            &spec(),
            json!({"pattern": "^cat", "replacement": "dog", "regex": true, "dryRun": false}),
        )
        .await;
        assert_eq!(fx.read("A.md"), "Cat\ndog\n");
    }

    #[tokio::test]
    async fn leaves_frontmatter_and_fenced_code_alone_unless_asked() {
        let raw = "---\ntitle: old\n---\nold\n```\nold\n```\n`old`\n";
        let fx = setup(&[("A.md", raw)]).await;
        fx.ok(
            &spec(),
            json!({"pattern": "old", "replacement": "new", "dryRun": false}),
        )
        .await;
        assert_eq!(
            fx.read("A.md"),
            "---\ntitle: old\n---\nnew\n```\nold\n```\n`new`\n"
        );
        fx.ok(
            &spec(),
            json!({
                "pattern": "old",
                "replacement": "new",
                "includeFrontmatter": true,
                "includeCode": true,
                "dryRun": false,
            }),
        )
        .await;
        assert_eq!(
            fx.read("A.md"),
            "---\ntitle: new\n---\nnew\n```\nnew\n```\n`new`\n"
        );
    }

    #[tokio::test]
    async fn a_match_never_spans_into_a_code_fence() {
        let fx = setup(&[("A.md", "x\n```\ny\n```\n")]).await;
        let out = fx
            .ok(
                &spec(),
                json!({"pattern": "x\\s+```", "replacement": "", "regex": true}),
            )
            .await;
        assert_eq!(out["filesMatched"], 0);
    }

    #[tokio::test]
    async fn restricts_to_a_folder() {
        let fx = setup(&[("a/A.md", "hit"), ("b/B.md", "hit")]).await;
        let out = fx
            .ok(
                &spec(),
                json!({"pattern": "hit", "replacement": "x", "folder": "a"}),
            )
            .await;
        assert_eq!(
            out["files"],
            json!([{"path": "a/A.md", "matches": 1, "samples": [{"line": 1, "before": "hit", "after": "x"}]}])
        );
    }

    #[tokio::test]
    async fn max_files_truncates_a_dry_run_and_refuses_a_write_without_touching_files() {
        let fx = setup(&[("A.md", "hit"), ("B.md", "hit"), ("C.md", "hit")]).await;
        let dry = fx
            .ok(
                &spec(),
                json!({"pattern": "hit", "replacement": "x", "maxFiles": 2}),
            )
            .await;
        assert_eq!(dry["filesMatched"], 3);
        assert_eq!(dry["truncated"], true);
        assert_eq!(dry["files"].as_array().unwrap().len(), 2);

        let err = fx
            .err(
                &spec(),
                json!({"pattern": "hit", "replacement": "x", "maxFiles": 2, "dryRun": false}),
            )
            .await;
        assert_eq!(
            err,
            "3 files match, more than maxFiles (2). Nothing was written. Narrow `folder` or raise `maxFiles`."
        );
        assert_eq!(fx.read("A.md"), "hit");
    }

    #[tokio::test]
    async fn rejects_unsafe_invalid_over_long_and_empty_patterns() {
        let fx = setup(&[("A.md", "aaa")]).await;
        let long = "a".repeat(501);
        for (pattern, msg) in [
            ("(a+)+", "quantified group"),
            ("[", "Invalid regex"),
            (long.as_str(), "too long"),
            ("", "at least 1 character"),
        ] {
            let err = fx
                .err(
                    &spec(),
                    json!({"pattern": pattern, "replacement": "x", "regex": true}),
                )
                .await;
            assert!(err.contains(msg), "{pattern}: {err}");
        }
        assert_eq!(fx.read("A.md"), "aaa");
    }

    #[tokio::test]
    async fn samples_center_on_the_match_in_a_long_line_and_group_matches_per_line() {
        let long = format!("{} hit {}", "x".repeat(200), "y".repeat(200));
        let fx = setup(&[("A.md", &format!("{long}\nhit\nhit\nhit\nhit"))]).await;
        let out = fx
            .ok(&spec(), json!({"pattern": "hit", "replacement": "HIT"}))
            .await;
        let samples = out["files"][0]["samples"].as_array().unwrap();
        let first = &samples[0];
        assert_eq!(first["before"].as_str().unwrap().len(), 80 + 3 + 80);
        assert!(first["after"].as_str().unwrap().contains(" HIT "));
        let lines: Vec<_> = samples[1..].iter().map(|s| s["line"].clone()).collect();
        assert_eq!(lines, [json!(2), json!(3)]);
    }

    #[tokio::test]
    async fn sample_context_counts_utf16_units() {
        // 100 astral characters before the match: 80 units are 40 of them.
        let fx = setup(&[("A.md", &format!("{}hit", "🎉".repeat(100)))]).await;
        let out = fx
            .ok(&spec(), json!({"pattern": "hit", "replacement": "x"}))
            .await;
        let sample = &out["files"][0]["samples"][0];
        assert_eq!(sample["before"], format!("{}hit", "🎉".repeat(40)));
        assert_eq!(sample["after"], format!("{}x", "🎉".repeat(40)));
    }

    #[tokio::test]
    async fn reports_a_failed_write_and_keeps_going() {
        let fx = setup(&[("a/A.md", "hit"), ("B.md", "hit")]).await;
        let out = {
            let _guard = Chmod::new(&fx.path("a"), 0o500);
            fx.ok(
                &spec(),
                json!({"pattern": "hit", "replacement": "x", "dryRun": false}),
            )
            .await
        };
        assert_eq!(out["filesChanged"], 1);
        let failed = out["failed"].as_array().unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0]["path"], "a/A.md");
        assert!(
            failed[0]["error"]
                .as_str()
                .unwrap()
                .starts_with("EACCES: permission denied, open '"),
            "{}",
            failed[0]["error"]
        );
        assert_eq!(fx.read("B.md"), "x");
    }
}
