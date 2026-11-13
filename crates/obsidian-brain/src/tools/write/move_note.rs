//! `move_note`: rename or move a note on disk. Every note that linked to
//! it has its wiki-links rewritten in place, so the vault stays consistent
//! without waiting for the next reindex.

use std::path::Path;

use anyhow::{Result, anyhow};
use rusqlite::Connection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::mcp::tool::ToolSpec;
use crate::resolve::resolve_single_note;
use crate::store::edges::get_edges_by_target;
use crate::store::nodes::{all_node_ids, get_node, migrate_stub_to_real, prune_orphan_stubs};
use crate::store::rename::rename_node;
use crate::store::sync::set_sync_mtime;
use crate::tools::support::run_background_reindex;
use crate::util::errors::node_fs_error_message;
use crate::util::js::basename_md;
use crate::vault::mover::move_note;
use crate::vault::vault_path::{read_utf8, resolve_vault_path, write_file_atomic};
use crate::vault::wiki_links::rewrite_wiki_links;

const DESCRIPTION: &str = "Rename or move a note. Inbound wiki-links in other notes are rewritten in place immediately (bare [[old]], [[old|alias]], ![[old]] embeds, and [[old#heading]]/[[old^block]] suffixes all handled). If the note's frontmatter has a `title:` field matching the old basename, it's auto-rewritten to the new basename (custom titles and missing titles are left alone). Response includes `linksRewritten: { files, occurrences }`.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Current path or fuzzy match of the note to move.
    source: String,
    /// New vault-relative path (including `.md`). `.md` is appended automatically if omitted.
    #[schemars(length(min = 1))]
    destination: String,
    /// If true, report what would be rewritten without mutating any files.
    dry_run: Option<bool>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("move_note", DESCRIPTION, |ctx, args| async move {
        let dry_run = args.dry_run == Some(true);
        let job = ctx.clone();
        let out = ctx
            .db
            .run(move |conn| -> Result<Value> {
                let vault = &job.config.vault_path;
                let file_rel_path = resolve_single_note(&args.source, conn, false)?;

                if dry_run {
                    // `move_note`'s rule for a destination without an
                    // extension: append `.md` when the last segment has no dot.
                    let last = args.destination.rsplit(['/', '\\']).next().unwrap_or("");
                    let new_path = if last.contains('.') {
                        args.destination.clone()
                    } else {
                        format!("{}.md", args.destination)
                    };
                    // The real move would refuse a destination outside the vault.
                    resolve_vault_path(vault, &new_path)?;
                    let links = preview_inbound_rewrites(conn, vault, &file_rel_path, &new_path)?;
                    let total_occurrences: usize = links.iter().map(|l| l.occurrences).sum();
                    return Ok(json!({
                        "dryRun": true,
                        "oldPath": file_rel_path,
                        "newPath": new_path,
                        "linksToRewrite": links,
                        "totalFiles": links.len(),
                        "totalOccurrences": total_occurrences,
                    }));
                }

                let result = move_note(vault, &file_rel_path, &args.destination)?;

                // Rewrite the sources on disk first: it reads the edges keyed
                // on the old path, which `rename_node` then repoints.
                let rewritten =
                    rewrite_inbound_links(conn, vault, &result.old_path, &result.new_path)?;

                // Every row keyed on the old path moves to the new one, so the
                // inbound edges survive instead of being re-derived.
                rename_node(conn, &result.old_path, &result.new_path)?;

                // A stub for the old stem that still has inbound edges hands
                // them to the renamed note, so no ghost edge is left.
                let old_stem = basename_md(&result.old_path).to_string();
                let stub_path = format!("_stub/{old_stem}.md");
                if get_node(conn, &stub_path)?.is_some() {
                    migrate_stub_to_real(conn, &stub_path, &result.new_path)?;
                }

                // Make the next reindex reparse every rewritten source. With
                // 1-second mtimes a fast rewrite can share the recorded
                // second and be skipped.
                for src in &rewritten.rewritten_sources {
                    set_sync_mtime(conn, src, 0.0, None)?;
                }

                let prefix = format!("_stub/{old_stem}");
                let candidates: Vec<String> = all_node_ids(conn)?
                    .into_iter()
                    .filter(|id| id.starts_with(&prefix))
                    .collect();
                let stubs_pruned = prune_orphan_stubs(conn, &candidates)?;

                Ok(json!({
                    "oldPath": result.old_path,
                    "newPath": result.new_path,
                    "linksRewritten": {
                        "files": rewritten.files,
                        "occurrences": rewritten.occurrences,
                    },
                    "stubsPruned": stubs_pruned,
                }))
            })
            .await?;

        if !dry_run {
            run_background_reindex(&ctx);
        }
        Ok(out)
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct LinkPreview {
    file: String,
    occurrences: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct InboundRewrite {
    pub files: usize,
    pub occurrences: usize,
    pub rewritten_sources: Vec<String>,
}

/// The notes that link to `old_path`, in edge order and once each. Edges
/// that still target the `_stub/<old stem>.md` stub count too: a forward
/// reference that was never migrated keeps its edges there.
fn inbound_sources(conn: &Connection, old_path: &str, new_path: &str) -> Result<Vec<String>> {
    let stub_path = format!("_stub/{}.md", basename_md(old_path));
    let mut sources: Vec<String> = Vec::new();
    for edge in get_edges_by_target(conn, old_path)?
        .into_iter()
        .chain(get_edges_by_target(conn, &stub_path)?)
    {
        if !sources.contains(&edge.source_id) {
            sources.push(edge.source_id);
        }
    }
    // A note cannot break a link to itself by moving; skip it anyway.
    sources.retain(|s| s != old_path && s != new_path);
    Ok(sources)
}

/// A source file to read, or `None` when it is gone or outside the vault.
fn read_source(vault: &Path, source_rel: &str) -> Option<(std::path::PathBuf, String)> {
    let abs = resolve_vault_path(vault, source_rel).ok()?.abs;
    let content = read_utf8(&abs).ok()?;
    Some((abs, content))
}

/// What [`rewrite_inbound_links`] would do, without writing.
fn preview_inbound_rewrites(
    conn: &Connection,
    vault: &Path,
    old_path: &str,
    new_path: &str,
) -> Result<Vec<LinkPreview>> {
    let mut results = Vec::new();
    for source in inbound_sources(conn, old_path, new_path)? {
        let Some((_, content)) = read_source(vault, &source) else {
            continue;
        };
        let occurrences = rewrite_wiki_links(&content, old_path, new_path).occurrences;
        if occurrences > 0 {
            results.push(LinkPreview {
                file: source,
                occurrences,
            });
        }
    }
    Ok(results)
}

/// Rewrite the wiki-links of every note that linked to `old_path` before
/// the move. A source missing on disk is skipped: the index may lag an
/// out-of-band delete, and that must not fail the move.
pub(super) fn rewrite_inbound_links(
    conn: &Connection,
    vault: &Path,
    old_path: &str,
    new_path: &str,
) -> Result<InboundRewrite> {
    let mut out = InboundRewrite::default();
    for source in inbound_sources(conn, old_path, new_path)? {
        let Some((abs, content)) = read_source(vault, &source) else {
            continue;
        };
        let rewrite = rewrite_wiki_links(&content, old_path, new_path);
        if rewrite.occurrences > 0 {
            write_file_atomic(&abs, &rewrite.text)
                .map_err(|e| anyhow!(node_fs_error_message(&e, "open", &abs, None)))?;
            out.files += 1;
            out.occurrences += rewrite.occurrences;
            out.rewritten_sources.push(source);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    //! Ported from `test/tools/move-note/{rewrite-inbound-links,
    //! ghost-link-fix,rename-node}.test.ts`.

    use super::*;
    use crate::store::Db;
    use crate::store::edges::insert_edge;
    use crate::store::nodes::upsert_node;
    use crate::types::{ParsedEdge, ParsedNode};
    use serde_json::Map;

    struct Fixture {
        dir: tempfile::TempDir,
        db: Db,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
                db: Db::open_in_memory().unwrap(),
            }
        }
        fn vault(&self) -> &Path {
            self.dir.path()
        }
        fn file(&self, rel: &str, content: &str) {
            std::fs::write(self.vault().join(rel), content).unwrap();
        }
        fn read(&self, rel: &str) -> String {
            std::fs::read_to_string(self.vault().join(rel)).unwrap()
        }
        fn node(&self, id: &str, title: &str) {
            let mut fm = Map::new();
            if id.starts_with("_stub/") {
                fm.insert("_stub".into(), true.into());
            }
            self.db.with(|c| {
                upsert_node(
                    c,
                    &ParsedNode {
                        id: id.into(),
                        title: title.into(),
                        content: String::new(),
                        frontmatter: fm,
                    },
                )
                .unwrap()
            });
        }
        fn edge(&self, source: &str, target: &str, context: &str) {
            self.db.with(|c| {
                insert_edge(
                    c,
                    &ParsedEdge {
                        source_id: source.into(),
                        target_id: target.into(),
                        context: context.into(),
                        target_subpath: None,
                    },
                )
                .unwrap()
            });
        }
        fn move_and_rewrite(&self, from: &str, to: &str) -> (String, String, InboundRewrite) {
            let moved = move_note(self.vault(), from, to).unwrap();
            let report = self.db.with(|c| {
                rewrite_inbound_links(c, self.vault(), &moved.old_path, &moved.new_path).unwrap()
            });
            (moved.old_path, moved.new_path, report)
        }
    }

    fn report(files: usize, occurrences: usize, sources: &[&str]) -> InboundRewrite {
        InboundRewrite {
            files,
            occurrences,
            rewritten_sources: sources.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn rewrites_a_single_inbound_link() {
        let f = Fixture::new();
        f.file("target.md", "# Target\n");
        f.file("source.md", "See [[target]] for context.\n");
        f.node("target.md", "Target");
        f.node("source.md", "Source");
        f.edge("source.md", "target.md", "link");
        let (_, new_path, r) = f.move_and_rewrite("target.md", "renamed.md");
        assert_eq!(new_path, "renamed.md");
        assert_eq!(r, report(1, 1, &["source.md"]));
        assert_eq!(f.read("source.md"), "See [[renamed]] for context.\n");
    }

    #[test]
    fn reports_zero_when_nothing_links_to_the_note() {
        let f = Fixture::new();
        f.file("lonely.md", "# Lonely\n");
        f.node("lonely.md", "Lonely");
        let (_, _, r) = f.move_and_rewrite("lonely.md", "still-lonely.md");
        assert_eq!(r, report(0, 0, &[]));
    }

    #[test]
    fn rewrites_every_link_shape_in_one_source() {
        let f = Fixture::new();
        f.file("target.md", "# Target\n");
        f.file(
            "source.md",
            "Plain [[target]], embed ![[target]], and alias [[target|see target]] done.\n",
        );
        f.node("target.md", "Target");
        f.node("source.md", "Source");
        f.edge("source.md", "target.md", "link");
        f.edge("source.md", "target.md", "embed");
        f.edge("source.md", "target.md", "link");
        let (_, _, r) = f.move_and_rewrite("target.md", "renamed.md");
        assert_eq!(r, report(1, 3, &["source.md"]));
        assert_eq!(
            f.read("source.md"),
            "Plain [[renamed]], embed ![[renamed]], and alias [[renamed|see target]] done.\n"
        );
    }

    #[test]
    fn rewrites_links_across_several_sources() {
        let f = Fixture::new();
        f.file("target.md", "# Target\n");
        f.file("a.md", "Refs [[target]] here.\n");
        f.file("b.md", "And [[target#Intro]] there.\n");
        f.node("target.md", "Target");
        f.node("a.md", "A");
        f.node("b.md", "B");
        f.edge("a.md", "target.md", "link");
        f.edge("b.md", "target.md", "link");
        let (_, _, r) = f.move_and_rewrite("target.md", "renamed.md");
        assert_eq!((r.files, r.occurrences), (2, 2));
        let mut sources = r.rewritten_sources.clone();
        sources.sort();
        assert_eq!(sources, ["a.md", "b.md"]);
        assert_eq!(f.read("a.md"), "Refs [[renamed]] here.\n");
        assert_eq!(f.read("b.md"), "And [[renamed#Intro]] there.\n");
    }

    #[test]
    fn a_move_that_keeps_the_stem_rewrites_nothing() {
        let f = Fixture::new();
        f.file("keep.md", "# keep\n");
        f.file("source.md", "Points at [[keep]].\n");
        f.node("keep.md", "keep");
        f.node("source.md", "Source");
        f.edge("source.md", "keep.md", "link");
        let (_, _, r) = f.move_and_rewrite("keep.md", "Archive/keep.md");
        assert_eq!(r, report(0, 0, &[]));
        assert_eq!(f.read("source.md"), "Points at [[keep]].\n");
    }

    #[test]
    fn a_source_missing_on_disk_is_skipped() {
        let f = Fixture::new();
        f.file("target.md", "# Target\n");
        f.node("target.md", "Target");
        f.node("ghost.md", "Ghost");
        f.edge("ghost.md", "target.md", "link");
        let (_, _, r) = f.move_and_rewrite("target.md", "renamed.md");
        assert_eq!(r, report(0, 0, &[]));
    }

    #[test]
    fn sources_that_link_through_the_old_stub_are_rewritten() {
        let f = Fixture::new();
        f.file("BMW.md", "# BMW\n");
        f.file("Cars.md", "I drive a [[BMW]] every day.\n");
        f.node("BMW.md", "BMW");
        f.node("Cars.md", "Cars");
        f.node("_stub/BMW.md", "BMW");
        f.edge("Cars.md", "_stub/BMW.md", "link");
        let (_, _, r) = f.move_and_rewrite("BMW.md", "BMW & Audi.md");
        assert_eq!(r, report(1, 1, &["Cars.md"]));
        assert_eq!(f.read("Cars.md"), "I drive a [[BMW & Audi]] every day.\n");
    }

    #[test]
    fn a_source_with_real_and_stub_edges_is_rewritten_once() {
        let f = Fixture::new();
        f.file("target.md", "# Target\n");
        f.file(
            "source.md",
            "Real-target [[target]] and stub-target [[target]] again.\n",
        );
        f.node("target.md", "Target");
        f.node("source.md", "Source");
        f.edge("source.md", "target.md", "link");
        f.node("_stub/target.md", "target");
        f.edge("source.md", "_stub/target.md", "link");
        let (_, _, r) = f.move_and_rewrite("target.md", "renamed.md");
        assert_eq!(r, report(1, 2, &["source.md"]));
    }

    #[test]
    fn inbound_edges_survive_rewrite_and_rename_without_a_reparse() {
        let f = Fixture::new();
        f.file("target.md", "# Target\n");
        f.file("a.md", "See [[target]] today.\n");
        f.file("b.md", "Reference to [[target]].\n");
        f.node("target.md", "Target");
        f.node("a.md", "A");
        f.node("b.md", "B");
        f.edge("a.md", "target.md", "see");
        f.edge("b.md", "target.md", "ref");
        let (old, new, _) = f.move_and_rewrite("target.md", "renamed.md");
        f.db.with(|c| rename_node(c, &old, &new)).unwrap();
        let (on_old, on_new) = f.db.with(|c| {
            (
                get_edges_by_target(c, "target.md").unwrap(),
                get_edges_by_target(c, "renamed.md").unwrap(),
            )
        });
        assert!(on_old.is_empty());
        let mut sources: Vec<_> = on_new.iter().map(|e| e.source_id.as_str()).collect();
        sources.sort();
        assert_eq!(sources, ["a.md", "b.md"]);
        let mut contexts: Vec<_> = on_new.iter().map(|e| e.context.as_str()).collect();
        contexts.sort();
        assert_eq!(contexts, ["ref", "see"]);
        assert_eq!(f.read("a.md"), "See [[renamed]] today.\n");
        assert_eq!(f.read("b.md"), "Reference to [[renamed]].\n");
    }
}
