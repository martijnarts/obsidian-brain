//! `rename_tag`: rename a tag in note bodies and in the `tags` / `tag`
//! frontmatter keys. Tags match case-insensitively and as whole tags, so
//! `#old` never rewrites `#older`; with `include_nested`, `#old/child`
//! becomes `#new/child`. Code, URLs and headings are left alone.

use std::sync::{Arc, LazyLock};

use anyhow::{Result, bail};
use fancy_regex::Regex;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{blocking, write_note};
use crate::context::VaultContext;
use crate::mcp::tool::ToolSpec;
use crate::tools::support::background_reindex::run_background_reindex;
use crate::util::errors::error_message;
use crate::util::js;
use crate::vault::scan::{list_note_paths, mask_code, split_frontmatter};
use crate::vault::vault_path::{read_note_file, resolve_vault_path};

const DESCRIPTION: &str = "Rename a tag across the vault: inline `#tag` (not in code, URLs or headings) and frontmatter `tags`/`tag`. Dry run by default: returns per-file counts; pass `dryRun: false` to write.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Tag to rename, `#` optional. Case-insensitive.
    #[schemars(length(min = 1))]
    from: String,
    /// New tag name, `#` optional.
    #[schemars(length(min = 1))]
    to: String,
    /// Also rename `#from/child` to `#to/child`. Default true.
    include_nested: Option<bool>,
    /// Default true. Pass false to write.
    dry_run: Option<bool>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("rename_tag", DESCRIPTION, |ctx, args| async move {
        blocking(ctx, move |ctx| run(ctx, args)).await
    })
}

fn run(ctx: &Arc<VaultContext>, args: Args) -> Result<Value> {
    let from = normalize_tag(&args.from);
    let to = normalize_tag(&args.to);
    for t in [from, to] {
        if !is_valid_tag(t) {
            bail!("Invalid tag \"{t}\": use letters, digits, _, - and /, not only digits.");
        }
    }
    if from == to {
        bail!("`from` and `to` are the same tag.");
    }
    let dry_run = args.dry_run != Some(false);
    let nested = args.include_nested != Some(false);
    let vault = &ctx.config.vault_path;
    let renamer = TagRenamer::new(from, to, nested)?;

    let mut files = Vec::new();
    let mut failed = Vec::new();
    let mut replacements = 0;
    let paths = ctx.db.with(|conn| list_note_paths(conn, None, &[]))?;
    for path in paths {
        let Ok(note) = read_note_file(vault, &path) else {
            continue;
        };
        let res = renamer.rename_in_note(&note.content);
        if res.inline + res.frontmatter == 0 {
            continue;
        }
        if !dry_run {
            let written = resolve_vault_path(vault, &path)
                .map_err(|e| error_message(&e))
                .and_then(|p| write_note(&p.abs, &res.text));
            if let Err(error) = written {
                failed.push(json!({"path": path, "error": error}));
                continue;
            }
        }
        replacements += res.inline + res.frontmatter;
        files.push(json!({"path": path, "inline": res.inline, "frontmatter": res.frontmatter}));
    }

    // Fire-and-forget reindex, once for the whole batch.
    if !dry_run && !files.is_empty() {
        run_background_reindex(ctx);
    }

    let mut out = Map::new();
    out.insert("dryRun".into(), dry_run.into());
    out.insert("from".into(), from.into());
    out.insert("to".into(), to.into());
    out.insert("filesMatched".into(), files.len().into());
    out.insert("replacements".into(), replacements.into());
    out.insert("files".into(), Value::Array(files));
    if !failed.is_empty() {
        out.insert("failed".into(), Value::Array(failed));
    }
    Ok(Value::Object(out))
}

/// The tag without surrounding whitespace and leading `#`s.
pub fn normalize_tag(tag: &str) -> &str {
    js::trim(tag).trim_start_matches('#')
}

static TAG_CHARS: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[\p{L}\p{N}_\-/]+$").expect("valid pattern"));
static DIGITS: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\p{N}+$").expect("valid pattern"));

pub fn is_valid_tag(tag: &str) -> bool {
    TAG_CHARS.is_match(tag)
        && !DIGITS.is_match(tag)
        && !tag.starts_with('/')
        && !tag.ends_with('/')
        && !tag.contains("//")
}

/// Lookahead that ends a whole tag; `/` continues a nested one.
fn tag_end(nested: bool) -> String {
    format!(r"(?![\p{{L}}\p{{N}}_\-{}])", if nested { "" } else { "/" })
}

// The `tags:` / `tag:` key line and a block list item. The key matches
// ASCII letters in any case, as JS `i` without `u` does; `.` stops at the
// JS line terminators.
static KEY_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"\A([Tt][Aa][Gg][Ss]?[ \t]*:)({}*)\z", js::DOT)).expect("valid pattern")
});
static LIST_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"\A([ \t]*-[ \t]+)({}*)\z", js::DOT)).expect("valid pattern")
});
static BLANK_OR_COMMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"\A[ \t]*(?:#{}*)?\r?\z", js::DOT)).expect("valid pattern")
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Renamed {
    pub text: String,
    pub inline: usize,
    pub frontmatter: usize,
}

/// The compiled patterns of one rename.
pub struct TagRenamer {
    to: String,
    inline: Regex,
    value: Regex,
}

impl TagRenamer {
    pub fn new(from: &str, to: &str, nested: bool) -> Result<Self> {
        let from = fancy_regex::escape(from);
        let end = tag_end(nested);
        // A tag starts after whitespace or at the start of the body, which
        // rules out URL fragments (`x#old`) and `##` runs; a heading's `# `
        // has no tag character after the `#`.
        let inline = Regex::new(&format!(r"(?i)(?:\A|(?<=[{}]))#{from}{end}", js::WS))?;
        let value = Regex::new(&format!(
            r#"(?i)(?:\A|(?<=[{},\[\]"']))(#?){from}{end}"#,
            js::WS
        ))?;
        Ok(Self {
            to: to.to_string(),
            inline,
            value,
        })
    }

    pub fn rename_in_note(&self, raw: &str) -> Renamed {
        let split = split_frontmatter(raw);
        let (fm_text, fm_count) = self.rename_in_frontmatter(split.frontmatter);
        let body = split.body;

        // Matching on the code mask skips code; the mask keeps offsets.
        let masked = mask_code(body, false);
        let mut inline = 0;
        let mut out = fm_text;
        let mut at = 0;
        for m in self.inline.find_iter(&masked).map_while(|m| m.ok()) {
            out.push_str(&body[at..m.start()]);
            out.push('#');
            out.push_str(&self.to);
            at = m.end();
            inline += 1;
        }
        out.push_str(&body[at..]);
        Renamed {
            text: out,
            inline,
            frontmatter: fm_count,
        }
    }

    /// Rename the tag inside the `tags` / `tag` frontmatter keys only, line
    /// by line, so the rest of the YAML keeps its exact bytes. Handles an
    /// inline value (`tags: a, old` or `tags: [a, "#old"]`) and a block
    /// list (`tags:` followed by `- old` items).
    pub fn rename_in_frontmatter(&self, frontmatter: &str) -> (String, usize) {
        if frontmatter.is_empty() {
            return (String::new(), 0);
        }
        let mut count = 0;
        let mut rename = |s: &str| -> String {
            let mut out = String::with_capacity(s.len());
            let mut at = 0;
            for c in self.value.captures_iter(s).map_while(|c| c.ok()) {
                let m = c.get(0).expect("group 0");
                out.push_str(&s[at..m.start()]);
                out.push_str(c.get(1).map_or("", |g| g.as_str()));
                out.push_str(&self.to);
                at = m.end();
                count += 1;
            }
            out.push_str(&s[at..]);
            out
        };

        let mut in_list = false;
        let lines: Vec<String> = frontmatter
            .split('\n')
            .map(|line| {
                if let Ok(Some(key)) = KEY_LINE.captures(line) {
                    let value = &key[2];
                    in_list = js::trim(value).is_empty();
                    return format!("{}{}", &key[1], rename(value));
                }
                if !in_list {
                    return line.to_string();
                }
                if let Ok(Some(item)) = LIST_ITEM.captures(line) {
                    return format!("{}{}", &item[1], rename(&item[2]));
                }
                if !BLANK_OR_COMMENT.is_match(line).unwrap_or(false) {
                    in_list = false;
                }
                line.to_string()
            })
            .collect();
        (lines.join("\n"), count)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::test_support::{Chmod, Fixture};
    use super::*;

    fn rename(raw: &str, from: &str, to: &str, nested: bool) -> Renamed {
        TagRenamer::new(from, to, nested)
            .unwrap()
            .rename_in_note(raw)
    }

    fn rename_fm(fm: &str, from: &str, to: &str, nested: bool) -> (String, usize) {
        TagRenamer::new(from, to, nested)
            .unwrap()
            .rename_in_frontmatter(fm)
    }

    #[test]
    fn normalize_tag_and_is_valid_tag() {
        assert_eq!(normalize_tag(" ##proj "), "proj");
        assert!(is_valid_tag("proj/a-b_c"));
        assert!(is_valid_tag("ünï"));
        for bad in ["", "123", "/a", "a/", "a//b", "a b", "a#b"] {
            assert!(!is_valid_tag(bad), "{bad}");
        }
    }

    #[test]
    fn renames_whole_inline_tags_only_nested_by_default_case_insensitively() {
        let body = "#old #Old #older #old/child #old-x x#old a/#old\n#old at start";
        assert_eq!(
            rename(body, "old", "new", true),
            Renamed {
                text: "#new #new #older #new/child #old-x x#old a/#old\n#new at start".into(),
                inline: 4,
                frontmatter: 0,
            }
        );
        assert_eq!(
            rename("#old/child #old", "old", "new", false).text,
            "#old/child #new"
        );
    }

    #[test]
    fn skips_code_urls_and_headings() {
        let body = "# old\n## old heading\n`#old`\n```\n#old\n```\nhttp://x.com/#old http://x#old\n[l](https://a.b/c#old)\n#old";
        let res = rename(body, "old", "new", true);
        assert_eq!(res.inline, 1);
        assert!(res.text.ends_with("\n#new"));
        assert_eq!(&res.text[..res.text.len() - 4], &body[..body.len() - 4]);
    }

    #[test]
    fn renames_frontmatter_tag_lists_strings_and_flow_lists_keeping_other_bytes() {
        let fm = [
            "---",
            "title: old  # keep",
            "tags:",
            "  - old",
            "  - \"#old/sub\"",
            "  - 'older'",
            "",
            "  # comment",
            "  - Old",
            "aliases:",
            "  - old",
            "tag: [a, \"#old\", old]",
            "Tags: one, old two",
            "---",
            "",
        ]
        .join("\n");
        let (text, count) = rename_fm(&fm, "old", "new", true);
        assert_eq!(count, 6);
        let want = fm
            .replacen("  - old\n  - \"#old/sub\"", "  - new\n  - \"#new/sub\"", 1)
            .replacen("  - Old", "  - new", 1)
            .replacen("tag: [a, \"#old\", old]", "tag: [a, \"#new\", new]", 1)
            .replacen("Tags: one, old two", "Tags: one, new two", 1);
        assert_eq!(text, want);
        assert_eq!(rename_fm("", "old", "new", true), (String::new(), 0));
    }

    #[test]
    fn handles_js_whitespace_case_folding_and_line_ends() {
        // A CRLF frontmatter key line never matches, so it is kept.
        let cases: &[(&str, &str, &str, &str, usize, usize)] = &[
            (
                "---\r\ntags: [old]\r\n---\r\n#old\r\n",
                "old",
                "new",
                "---\r\ntags: [old]\r\n---\r\n#new\r\n",
                1,
                0,
            ),
            (
                "#ſtar #Star #\u{212A}ey #key",
                "star",
                "x",
                "#x #x #\u{212A}ey #key",
                2,
                0,
            ),
            (
                "a\u{A0}#old b\u{FEFF}#old c\u{85}#old d\u{2028}#old",
                "old",
                "new",
                "a\u{A0}#new b\u{FEFF}#new c\u{85}#old d\u{2028}#new",
                3,
                0,
            ),
            (
                "---\ntags:\n  - old\n\u{2028}\n  - old\n---\n",
                "old",
                "new",
                "---\ntags:\n  - new\n\u{2028}\n  - old\n---\n",
                0,
                1,
            ),
            (
                "#Über #übersicht #über #ÜBER",
                "über",
                "neu",
                "#neu #übersicht #neu #neu",
                3,
                0,
            ),
        ];
        for (raw, from, to, text, inline, frontmatter) in cases {
            let got = rename(raw, from, to, true);
            assert_eq!(
                (got.text.as_str(), got.inline, got.frontmatter),
                (*text, *inline, *frontmatter),
                "{raw:?}"
            );
        }
    }

    #[tokio::test]
    async fn dry_run_by_default_per_file_counts_no_writes() {
        let fx = Fixture::new(&[
            ("A.md", "---\ntags: [old]\n---\n#old text"),
            ("B.md", "#other"),
        ])
        .await;
        let out = fx.ok(&spec(), json!({"from": "#old", "to": "new"})).await;
        assert_eq!(
            out,
            json!({
                "dryRun": true,
                "from": "old",
                "to": "new",
                "filesMatched": 1,
                "replacements": 2,
                "files": [{"path": "A.md", "inline": 1, "frontmatter": 1}],
            })
        );
        assert_eq!(fx.read("A.md"), "---\ntags: [old]\n---\n#old text");
        assert_eq!(fx.reindex_calls(), 0);
    }

    #[tokio::test]
    async fn writes_with_dry_run_false_and_reindexes_once() {
        let fx = Fixture::new(&[
            ("A.md", "---\ntags: [old]\n---\n#old text #old/x"),
            ("B.md", "#old"),
        ])
        .await;
        let out = fx
            .ok(
                &spec(),
                json!({"from": "old", "to": "area/new", "dryRun": false}),
            )
            .await;
        assert_eq!(out["replacements"], 4);
        assert_eq!(
            fx.read("A.md"),
            "---\ntags: [area/new]\n---\n#area/new text #area/new/x"
        );
        assert_eq!(fx.read("B.md"), "#area/new");
        assert_eq!(fx.reindex_calls(), 1);
    }

    #[tokio::test]
    async fn include_nested_false_leaves_child_tags() {
        let fx = Fixture::new(&[("A.md", "#old #old/x")]).await;
        fx.ok(
            &spec(),
            json!({"from": "old", "to": "new", "includeNested": false, "dryRun": false}),
        )
        .await;
        assert_eq!(fx.read("A.md"), "#new #old/x");
    }

    #[tokio::test]
    async fn rejects_invalid_and_identical_tags() {
        let fx = Fixture::new(&[]).await;
        let cases = [
            (json!({"from": "123", "to": "x"}), "Invalid tag \"123\""),
            (json!({"from": "a", "to": "b c"}), "Invalid tag \"b c\""),
            (json!({"from": "a", "to": "#a"}), "the same tag"),
            (
                json!({"from": "", "to": "a"}),
                "Too small: expected string to have >=1 characters",
            ),
        ];
        for (args, msg) in cases {
            let err = fx.err(&spec(), args).await;
            assert!(err.contains(msg), "{err}");
        }
    }

    #[tokio::test]
    async fn reports_a_failed_write_and_keeps_going() {
        let fx = Fixture::new(&[("a/A.md", "#old"), ("B.md", "#old")]).await;
        let out = {
            let _guard = Chmod::new(&fx.path("a"), 0o500);
            fx.ok(
                &spec(),
                json!({"from": "old", "to": "new", "dryRun": false}),
            )
            .await
        };
        assert_eq!(
            out["files"],
            json!([{"path": "B.md", "inline": 1, "frontmatter": 0}])
        );
        let failed = out["failed"].as_array().unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0]["path"], "a/A.md");
        assert!(failed[0]["error"].is_string());
    }
}
