//! `find_broken_links`: scan note bodies for `[[wikilinks]]` that do not
//! resolve: the target note is missing, or the `#heading` / `#^block`
//! subpath is missing from a target that exists. Resolution reuses the
//! parser's `resolve_link`, so a link reported here is a link the index
//! stores as a `_stub/` edge (or a subpath the target lacks).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::blocking;
use crate::context::VaultContext;
use crate::mcp::tool::ToolSpec;
use crate::util::js;
use crate::vault::scan::{
    find_block_ids, find_headings, find_wiki_links, heading_key, line_at, list_note_paths,
    split_frontmatter,
};
use crate::vault::vault_path::read_note_file;
use crate::vault::wiki_links::{build_stem_lookup, resolve_link};

const DESCRIPTION: &str = "List wikilinks whose target note is missing, or whose `#heading` / `#^block` is missing from the target. Read-only. Returns source path, 1-based line, link text and reason per broken link.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Only scan notes under this folder.
    folder: Option<String>,
    /// Folders to skip.
    exclude_folders: Option<Vec<String>>,
    /// Max broken links returned. Default 100.
    #[schemars(range(min = 1))]
    limit: Option<u64>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("find_broken_links", DESCRIPTION, |ctx, args| {
        blocking(ctx, move |ctx| run(ctx, args))
    })
}

struct Anchors {
    headings: HashSet<String>,
    blocks: BTreeSet<String>,
}

fn anchors_of(body: &str) -> Anchors {
    Anchors {
        headings: find_headings(body)
            .iter()
            .map(|h| heading_key(&h.text))
            .collect(),
        blocks: find_block_ids(body),
    }
}

static HAS_EXTENSION: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\.[A-Za-z0-9]+\z").expect("valid pattern"));

fn run(ctx: &VaultContext, args: Args) -> Result<Value> {
    let vault = &ctx.config.vault_path;
    let exclude = args.exclude_folders.unwrap_or_default();
    let (all_paths, sources) = ctx.db.with(|conn| -> Result<_> {
        Ok((
            list_note_paths(conn, None, &[])?,
            list_note_paths(conn, args.folder.as_deref(), &exclude)?,
        ))
    })?;
    let all_paths_set: HashSet<String> = all_paths.iter().cloned().collect();
    let stem_lookup = build_stem_lookup(&all_paths);
    let mut anchor_cache: HashMap<String, Option<Anchors>> = HashMap::new();
    let mut attachments: Option<HashSet<String>> = None;

    let mut broken: Vec<Value> = Vec::new();
    let mut scanned_files = 0;
    for source in &sources {
        let Ok(note) = read_note_file(vault, source) else {
            continue;
        };
        let raw = note.content;
        scanned_files += 1;
        let split = split_frontmatter(&raw);
        for link in find_wiki_links(split.body) {
            if link.embed {
                continue;
            }
            let mut report = |reason: &str| {
                broken.push(json!({
                    "source": source,
                    "line": line_at(&raw, split.frontmatter.len() + link.start),
                    "link": link.raw,
                    "reason": reason,
                }));
            };

            let target = if link.target.is_empty() {
                Some(source.clone())
            } else {
                let target = resolve_link(
                    &link.target,
                    &stem_lookup,
                    Some(&all_paths_set),
                    Some(source),
                    None,
                );
                if target.is_none() && !is_attachment(vault, &link.target, &mut attachments)? {
                    report("note_not_found");
                    continue;
                }
                target
            };
            let (Some(target), Some(subpath)) = (target, link.subpath.as_deref()) else {
                continue;
            };
            if js::trim(subpath).is_empty() {
                continue;
            }

            let anchors = anchor_cache.entry(target.clone()).or_insert_with(|| {
                if target == *source {
                    Some(anchors_of(split.body))
                } else {
                    read_note_file(vault, &target)
                        .ok()
                        .map(|n| anchors_of(split_frontmatter(&n.content).body))
                }
            });
            let Some(anchors) = anchors else {
                continue;
            };
            let parts: Vec<&str> = subpath
                .split('#')
                .map(js::trim)
                .filter(|p| !p.is_empty())
                .collect();
            let Some(last) = parts.last() else {
                continue;
            };
            if let Some(block) = last.strip_prefix('^') {
                if !anchors.blocks.contains(block) {
                    report("block_not_found");
                }
            } else if parts
                .iter()
                .any(|p| !anchors.headings.contains(&heading_key(p)))
            {
                report("heading_not_found");
            }
        }
    }

    let cap = args
        .limit
        .map_or(100, |n| usize::try_from(n).unwrap_or(usize::MAX));
    let total = broken.len();
    let mut out = Map::new();
    out.insert("scannedFiles".into(), scanned_files.into());
    out.insert("total".into(), total.into());
    if total > cap {
        out.insert("truncated".into(), true.into());
    }
    broken.truncate(cap);
    out.insert("brokenLinks".into(), Value::Array(broken));
    Ok(Value::Object(out))
}

fn is_attachment(
    vault: &Path,
    target: &str,
    attachments: &mut Option<HashSet<String>>,
) -> Result<bool> {
    if !HAS_EXTENSION.is_match(target) || target.ends_with(".md") {
        return Ok(false);
    }
    if attachments.is_none() {
        *attachments = Some(list_attachments(vault)?);
    }
    let set = attachments.as_ref().expect("filled above");
    let base = target.rsplit('/').next().unwrap_or(target);
    Ok(set.contains(target) || set.contains(base))
}

/// Every non-markdown entry in the vault, by vault-relative path and by
/// basename, skipping hidden folders. A wikilink to one of these (e.g.
/// `[[report.pdf]]`) is an attachment link, not a broken note link.
fn list_attachments(vault: &Path) -> Result<HashSet<String>> {
    let mut out = HashSet::new();
    for entry in walkdir::WalkDir::new(vault).min_depth(1) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(vault)?;
        let parts: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let joined = parts.join("/");
        if joined.ends_with(".md") || parts.iter().any(|p| p.starts_with('.')) {
            continue;
        }
        if let Some(last) = parts.last() {
            out.insert(last.clone());
        }
        out.insert(joined);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::test_support::Fixture;
    use super::spec;
    use crate::store::nodes::upsert_node;
    use crate::types::ParsedNode;

    async fn run(files: &[(&str, &str)], args: serde_json::Value) -> serde_json::Value {
        Fixture::new(files).await.ok(&spec(), args).await
    }

    fn links(out: &serde_json::Value) -> Vec<String> {
        out["brokenLinks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["link"].as_str().unwrap().to_string())
            .collect()
    }

    #[tokio::test]
    async fn returns_nothing_for_a_vault_whose_links_all_resolve() {
        let out = run(
            &[
                (
                    "A.md",
                    "See [[B]] and [[B#Intro]] and [[B#^blk]] and [[B|alias]].",
                ),
                ("B.md", "# Intro\n\nA paragraph ^blk\n"),
            ],
            json!({}),
        )
        .await;
        assert_eq!(
            out,
            json!({"scannedFiles": 2, "total": 0, "brokenLinks": []})
        );
    }

    #[tokio::test]
    async fn reports_a_missing_note_with_the_line_in_the_file_frontmatter_included() {
        let out = run(
            &[(
                "A.md",
                "---\ntitle: A\n---\nline one\nsee [[Missing|m]] here\n",
            )],
            json!({}),
        )
        .await;
        assert_eq!(
            out["brokenLinks"],
            json!([{"source": "A.md", "line": 5, "link": "[[Missing|m]]", "reason": "note_not_found"}])
        );
    }

    #[tokio::test]
    async fn reports_missing_headings_and_block_ids_in_an_existing_target() {
        let out = run(
            &[
                (
                    "A.md",
                    "[[B#Nope]]\n[[B#^gone]]\n[[B^gone2]]\n[[B#Real]]\n[[B#real]]",
                ),
                ("B.md", "## Real ##\n```\n# Fake\nx ^fakeblk\n```\n"),
            ],
            json!({}),
        )
        .await;
        let got: Vec<(String, String)> = out["brokenLinks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| {
                (
                    b["link"].as_str().unwrap().into(),
                    b["reason"].as_str().unwrap().into(),
                )
            })
            .collect();
        let want = [
            ("[[B#Nope]]", "heading_not_found"),
            ("[[B#^gone]]", "block_not_found"),
            ("[[B^gone2]]", "block_not_found"),
        ]
        .map(|(a, b)| (a.to_string(), b.to_string()));
        assert_eq!(got, want);
    }

    #[tokio::test]
    async fn checks_same_note_links_and_nested_heading_paths() {
        let out = run(
            &[(
                "A.md",
                "# Top\n## Sub\n[[#Top]] [[#Sub]] [[#Missing]] [[A#Top#Sub]] [[A#Top#Nope]] [[#^x]]",
            )],
            json!({}),
        )
        .await;
        assert_eq!(links(&out), ["[[#Missing]]", "[[A#Top#Nope]]", "[[#^x]]"]);
    }

    #[tokio::test]
    async fn skips_links_inside_code_embeds_and_attachments_that_exist_on_disk() {
        let out = run(
            &[
                (
                    "A.md",
                    "`[[Gone1]]`\n```\n[[Gone2]]\n```\n![[Gone3]]\n[[file.pdf]] [[sub/pic.png]] [[missing.pdf]] [[B#]]",
                ),
                ("file.pdf", "pdf"),
                ("assets/pic.png", "png"),
                (".hidden/missing.pdf", "x"),
                ("B.md", "b"),
            ],
            json!({}),
        )
        .await;
        assert_eq!(links(&out), ["[[missing.pdf]]"]);
    }

    #[tokio::test]
    async fn filters_by_folder_and_exclude_folders_and_applies_limit() {
        let out = run(
            &[
                ("a/One.md", "[[X1]] [[X2]] [[X3]]"),
                ("a/skip/Two.md", "[[X4]]"),
                ("b/Three.md", "[[X5]]"),
            ],
            json!({"folder": "/a/", "excludeFolders": ["a/skip"], "limit": 2}),
        )
        .await;
        assert_eq!(out["scannedFiles"], 1);
        assert_eq!(out["total"], 3);
        assert_eq!(out["truncated"], true);
        assert_eq!(out["brokenLinks"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn skips_a_file_that_vanished_or_links_outside_the_vault() {
        let fx = Fixture::new(&[("A.md", "[[Gone]]"), ("B.md", "[[Gone]]")]).await;
        std::fs::remove_file(fx.path("B.md")).unwrap();
        let outside = fx.dir.path().join("outside.md");
        std::fs::write(&outside, "[[Gone]]").unwrap();
        std::os::unix::fs::symlink(&outside, fx.path("C.md")).unwrap();
        fx.ctx.db.with(|conn| {
            upsert_node(
                conn,
                &ParsedNode {
                    id: "C.md".into(),
                    title: "C".into(),
                    content: String::new(),
                    frontmatter: Default::default(),
                },
            )
            .unwrap()
        });
        let out = fx.ok(&spec(), json!({})).await;
        assert_eq!(out["scannedFiles"], 1);
        let sources: Vec<&str> = out["brokenLinks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["source"].as_str().unwrap())
            .collect();
        assert_eq!(sources, ["A.md"]);
    }
}
