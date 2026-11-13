//! `rename_heading`: rename one heading in a note and rewrite every link to
//! it: `[[Note#Old]]`, aliased and embedded forms, `[[#Old]]` inside the
//! note itself, and heading paths such as `[[Note#Parent#Old]]`. Embeds
//! make no edges, so every note is scanned rather than only the
//! backlinkers. All files are planned before the first write.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};

use super::{blocking, require_non_empty, write_note};
use crate::context::VaultContext;
use crate::mcp::tool::ToolSpec;
use crate::resolve::single_note::resolve_single_note;
use crate::tools::support::background_reindex::run_background_reindex;
use crate::util::js;
use crate::vault::scan::{
    find_headings, find_wiki_links, heading_key, list_note_paths, split_frontmatter,
};
use crate::vault::vault_path::{read_note_file, resolve_vault_path};
use crate::vault::wiki_links::{build_stem_lookup, resolve_link};

const DESCRIPTION: &str = "Rename a heading in a note and rewrite every wikilink and embed that points to it across the vault. Errors if `from` is missing or `to` already exists in the note.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Path or fuzzy match of the note.
    name: String,
    /// Current heading text, without `#`.
    #[schemars(length(min = 1))]
    from: String,
    /// New heading text, without `#`.
    #[schemars(length(min = 1))]
    to: String,
    /// If true, report the changes without writing. Default false.
    dry_run: Option<bool>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("rename_heading", DESCRIPTION, |ctx, args| async move {
        require_non_empty("from", &args.from)?;
        require_non_empty("to", &args.to)?;
        blocking(ctx, move |ctx| run(ctx, args)).await
    })
}

struct Planned {
    path: String,
    text: String,
    links: usize,
}

fn run(ctx: &Arc<VaultContext>, args: Args) -> Result<Value> {
    let to = js::trim(&args.to);
    if to.contains(['#', '|', '^', '[', ']', '\n']) {
        bail!(
            "`to` cannot contain #, |, ^, [, ] or a line break, because links to it would break."
        );
    }
    let note_path = ctx
        .db
        .with(|conn| resolve_single_note(&args.name, conn, false))?;
    let vault = &ctx.config.vault_path;
    let note_raw = read_note_file(vault, &note_path)?.content;

    let split = split_frontmatter(&note_raw);
    let (frontmatter, body) = (split.frontmatter, split.body);
    let headings = find_headings(body);
    let from_key = heading_key(&args.from);
    let from_trimmed = js::trim(&args.from);
    let hits: Vec<_> = headings
        .iter()
        .filter(|h| js::trim(&h.text) == from_trimmed)
        .collect();
    if hits.is_empty() {
        bail!("Heading \"{}\" not found in {note_path}", args.from);
    }
    if hits.len() > 1 {
        let offset = frontmatter.split('\n').count() - 1;
        let lines = hits
            .iter()
            .map(|h| (h.line + offset).to_string())
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "Heading \"{}\" appears {} times in {note_path} (lines {lines}). Rename it by hand.",
            args.from,
            hits.len()
        );
    }
    let to_key = heading_key(to);
    if to_key != from_key && headings.iter().any(|h| heading_key(&h.text) == to_key) {
        bail!("Heading \"{to}\" already exists in {note_path}");
    }
    let heading = hits[0];
    let renamed_note = format!(
        "{frontmatter}{}{to}{}",
        &body[..heading.start],
        &body[heading.end..]
    );

    let all_paths = ctx.db.with(|conn| list_note_paths(conn, None, &[]))?;
    let all_paths_set: HashSet<String> = all_paths.iter().cloned().collect();
    let stem_lookup = build_stem_lookup(&all_paths);

    let mut planned = Vec::new();
    for path in &all_paths {
        let raw = if *path == note_path {
            renamed_note.clone()
        } else {
            match read_note_file(vault, path) {
                Ok(n) => n.content,
                Err(_) => continue,
            }
        };
        let (text, links) = rewrite_heading_links(&raw, &from_key, to, |target| {
            if target.is_empty() {
                *path == note_path
            } else {
                resolve_link(target, &stem_lookup, Some(&all_paths_set), Some(path), None)
                    .as_deref()
                    == Some(note_path.as_str())
            }
        });
        if *path == note_path || links > 0 {
            planned.push(Planned {
                path: path.clone(),
                text,
                links,
            });
        }
    }

    let dry_run = args.dry_run == Some(true);
    if !dry_run {
        for f in &planned {
            let abs = resolve_vault_path(vault, &f.path)?.abs;
            write_note(&abs, &f.text).map_err(|e| anyhow!(e))?;
        }
        // Fire-and-forget reindex, once for the whole batch.
        run_background_reindex(ctx);
    }

    let mut out = Map::new();
    if dry_run {
        out.insert("dryRun".into(), true.into());
    }
    out.insert("path".into(), note_path.into());
    out.insert("from".into(), heading.text.clone().into());
    out.insert("to".into(), to.into());
    out.insert(
        "filesChanged".into(),
        planned
            .iter()
            .map(|f| f.path.clone())
            .collect::<Vec<_>>()
            .into(),
    );
    out.insert(
        "linksRewritten".into(),
        planned.iter().map(|f| f.links).sum::<usize>().into(),
    );
    Ok(Value::Object(out))
}

/// Rewrite every heading-path segment equal to `from_key` in links whose
/// note part `points_at_note` accepts (`""` is a same-note link). Block
/// references are left alone. Returns the text and the links rewritten.
pub fn rewrite_heading_links(
    raw: &str,
    from_key: &str,
    to: &str,
    points_at_note: impl Fn(&str) -> bool,
) -> (String, usize) {
    let split = split_frontmatter(raw);
    let body = split.body;
    let mut links = 0;
    let mut out = String::from(split.frontmatter);
    let mut at = 0;
    for link in find_wiki_links(body) {
        let Some(subpath) = link.subpath.as_deref() else {
            continue;
        };
        if subpath.starts_with('^') || !points_at_note(&link.target) {
            continue;
        }
        let parts: Vec<&str> = subpath.split('#').collect();
        if !parts.iter().any(|p| heading_key(p) == from_key) {
            continue;
        }
        let new_subpath = parts
            .iter()
            .map(|p| if heading_key(p) == from_key { to } else { p })
            .collect::<Vec<_>>()
            .join("#");
        let open = if link.embed { 3 } else { 2 };
        let inner = &link.raw[open..link.raw.len() - 2];
        let hash = inner.find('#').expect("a heading subpath follows a #");
        let rest = &inner[hash + 1 + subpath.len()..];
        out.push_str(&body[at..link.start]);
        if link.embed {
            out.push('!');
        }
        out.push_str("[[");
        out.push_str(&inner[..=hash]);
        out.push_str(&new_subpath);
        out.push_str(rest);
        out.push_str("]]");
        at = link.end;
        links += 1;
    }
    out.push_str(&body[at..]);
    (out, links)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::test_support::Fixture;
    use super::*;

    const NOTE: &str =
        "---\ntitle: Note\n---\n# Parent\n## Old ##\nbody, see [[#Old]] and [[#Parent]]\n";

    #[tokio::test]
    async fn renames_the_heading_and_every_link_form_across_the_vault() {
        let fx = Fixture::new(&[
            ("Note.md", NOTE),
            (
                "A.md",
                "[[Note#Old]] [[Note#Old|alias]] ![[Note#Old]] [[Note#Parent#Old]] [[Note#old]]",
            ),
            (
                "B.md",
                "[[Note#Other]] [[Other#Old]] `[[Note#Old]]` [[Note#^Old]] [[#Old]]",
            ),
            ("Other.md", "## Old"),
        ])
        .await;
        let out = fx
            .ok(
                &spec(),
                json!({"name": "Note", "from": "Old", "to": "New Name"}),
            )
            .await;
        assert_eq!(
            out,
            json!({
                "path": "Note.md",
                "from": "Old",
                "to": "New Name",
                "filesChanged": ["A.md", "Note.md"],
                "linksRewritten": 6,
            })
        );
        assert_eq!(
            fx.read("Note.md"),
            "---\ntitle: Note\n---\n# Parent\n## New Name ##\nbody, see [[#New Name]] and [[#Parent]]\n"
        );
        assert_eq!(
            fx.read("A.md"),
            "[[Note#New Name]] [[Note#New Name|alias]] ![[Note#New Name]] [[Note#Parent#New Name]] [[Note#New Name]]"
        );
        assert_eq!(
            fx.read("B.md"),
            "[[Note#Other]] [[Other#Old]] `[[Note#Old]]` [[Note#^Old]] [[#Old]]"
        );
        assert_eq!(fx.reindex_calls(), 1);
    }

    #[tokio::test]
    async fn dry_run_reports_the_plan_and_writes_nothing() {
        let fx = Fixture::new(&[("Note.md", NOTE), ("A.md", "[[Note#Old]]")]).await;
        let out = fx
            .ok(
                &spec(),
                json!({"name": "Note.md", "from": "Old", "to": "New", "dryRun": true}),
            )
            .await;
        assert_eq!(out["dryRun"], true);
        assert_eq!(out["filesChanged"], json!(["A.md", "Note.md"]));
        assert_eq!(out["linksRewritten"], 2);
        assert_eq!(fx.read("Note.md"), NOTE);
        assert_eq!(fx.read("A.md"), "[[Note#Old]]");
        assert_eq!(fx.reindex_calls(), 0);
    }

    #[tokio::test]
    async fn a_parent_rename_rewrites_the_parent_segment_of_nested_links() {
        let fx = Fixture::new(&[("Note.md", NOTE), ("A.md", "[[Note#Parent#Old]]")]).await;
        fx.ok(
            &spec(),
            json!({"name": "Note", "from": "Parent", "to": "Top"}),
        )
        .await;
        assert_eq!(fx.read("A.md"), "[[Note#Top#Old]]");
    }

    #[tokio::test]
    async fn errors_on_a_missing_heading_a_duplicate_a_collision_and_a_bad_name() {
        let fx = Fixture::new(&[("Note.md", NOTE), ("Dup.md", "# Same\n# Same\n")]).await;
        let cases = [
            (
                json!({"name": "Note", "from": "Nope", "to": "X"}),
                "Heading \"Nope\" not found in Note.md",
            ),
            (
                json!({"name": "Note", "from": "Old", "to": "parent"}),
                "Heading \"parent\" already exists in Note.md",
            ),
            (
                json!({"name": "Note", "from": "Old", "to": "A#B"}),
                "`to` cannot contain",
            ),
            (
                json!({"name": "Dup", "from": "Same", "to": "X"}),
                "Heading \"Same\" appears 2 times in Dup.md (lines 1, 2). Rename it by hand.",
            ),
            (
                json!({"name": "Nowhere", "from": "Old", "to": "X"}),
                "No note found",
            ),
        ];
        for (args, msg) in cases {
            let err = fx.err(&spec(), args).await;
            assert!(err.contains(msg), "{err}");
        }
        assert_eq!(fx.read("Note.md"), NOTE);
    }

    #[tokio::test]
    async fn duplicate_lines_count_the_frontmatter() {
        let fx = Fixture::new(&[("Dup.md", "---\na: 1\n---\n# Same\n# Same\n")]).await;
        let err = fx
            .err(&spec(), json!({"name": "Dup", "from": "Same", "to": "X"}))
            .await;
        assert!(err.contains("(lines 4, 5)"), "{err}");
    }

    #[tokio::test]
    async fn errors_on_an_ambiguous_note_name() {
        let fx = Fixture::new(&[("a/Topic one.md", "# H"), ("b/Topic two.md", "# H")]).await;
        let err = fx
            .err(&spec(), json!({"name": "topic", "from": "H", "to": "X"}))
            .await;
        assert!(err.contains("Multiple notes match"), "{err}");
    }

    #[tokio::test]
    async fn allows_a_case_only_rename_of_the_same_heading() {
        let fx = Fixture::new(&[("Note.md", "# old\n[[#old]]")]).await;
        fx.ok(&spec(), json!({"name": "Note", "from": "old", "to": "Old"}))
            .await;
        assert_eq!(fx.read("Note.md"), "# Old\n[[#Old]]");
    }

    #[test]
    fn rewrite_heading_links_only_touches_links_the_predicate_accepts_and_keeps_frontmatter() {
        let raw = "---\nx: \"[[N#A]]\"\n---\n[[N#A]] [[M#A]] [[N]]";
        assert_eq!(
            rewrite_heading_links(raw, "a", "B", |t| t == "N"),
            (
                "---\nx: \"[[N#A]]\"\n---\n[[N#B]] [[M#A]] [[N]]".to_string(),
                1
            )
        );
    }
}
