//! `find_orphaned_notes`: notes that no other note links to. Works from the
//! edge table alone: a self-link does not count, and a link from an excluded
//! folder still counts (exclusion only hides notes from the result).

use std::collections::HashSet;

use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::blocking;
use crate::context::VaultContext;
use crate::mcp::tool::ToolSpec;
use crate::vault::scan::list_note_paths;

const DESCRIPTION: &str = "List notes with no incoming wikilinks from other notes (self-links do not count). Read-only. Returns path, title and outgoing link count per orphan.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Only report notes under this folder.
    folder: Option<String>,
    /// Folders whose notes are not reported. Their links still count.
    exclude_folders: Option<Vec<String>>,
    /// Max orphans returned. Default 100.
    #[schemars(range(min = 1))]
    limit: Option<u64>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("find_orphaned_notes", DESCRIPTION, |ctx, args| {
        blocking(ctx, move |ctx| run(ctx, args))
    })
}

fn run(ctx: &VaultContext, args: Args) -> Result<Value> {
    ctx.db.with(|conn| {
        let linked: HashSet<String> = conn
            .prepare("SELECT DISTINCT target_id FROM edges WHERE source_id != target_id")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut title_of = conn.prepare("SELECT title FROM nodes WHERE id = ?")?;
        let mut outgoing_of = conn.prepare(
            "SELECT COUNT(*) AS n FROM edges WHERE source_id = ? AND target_id != source_id",
        )?;

        let exclude = args.exclude_folders.unwrap_or_default();
        let orphans: Vec<String> = list_note_paths(conn, args.folder.as_deref(), &exclude)?
            .into_iter()
            .filter(|id| !linked.contains(id))
            .collect();
        let cap = args
            .limit
            .map_or(100, |n| usize::try_from(n).unwrap_or(usize::MAX));
        let mut listed = Vec::new();
        for path in orphans.iter().take(cap) {
            let title: String = title_of.query_row([path], |r| r.get(0))?;
            let outgoing: i64 = outgoing_of.query_row([path], |r| r.get(0))?;
            listed.push(json!({"path": path, "title": title, "outgoingLinks": outgoing}));
        }

        let mut out = Map::new();
        out.insert("total".into(), orphans.len().into());
        if orphans.len() > cap {
            out.insert("truncated".into(), true.into());
        }
        out.insert("orphans".into(), Value::Array(listed));
        Ok(Value::Object(out))
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::test_support::Fixture;
    use super::spec;

    async fn run(files: &[(&str, &str)], args: serde_json::Value) -> serde_json::Value {
        Fixture::new(files).await.ok(&spec(), args).await
    }

    fn paths(out: &serde_json::Value) -> Vec<&str> {
        out["orphans"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["path"].as_str().unwrap())
            .collect()
    }

    #[tokio::test]
    async fn lists_notes_without_incoming_links_self_links_and_stubs_do_not_count() {
        let out = run(
            &[
                (
                    "Hub.md",
                    "---\ntitle: The Hub\n---\n[[Leaf]] [[Hub]] [[Missing]] [[Leaf]]",
                ),
                ("Leaf.md", "no links"),
                ("Lonely.md", "[[Lonely]]"),
            ],
            json!({}),
        )
        .await;
        assert_eq!(
            out,
            json!({
                "total": 2,
                "orphans": [
                    {"path": "Hub.md", "title": "The Hub", "outgoingLinks": 3},
                    {"path": "Lonely.md", "title": "Lonely", "outgoingLinks": 0},
                ],
            })
        );
    }

    #[tokio::test]
    async fn hides_excluded_folders_from_the_result_but_counts_their_links() {
        let out = run(
            &[
                ("templates/T.md", "[[Used]]"),
                ("Used.md", ""),
                ("notes/Free.md", ""),
            ],
            json!({"excludeFolders": ["/templates/"]}),
        )
        .await;
        assert_eq!(paths(&out), ["notes/Free.md"]);
    }

    #[tokio::test]
    async fn filters_by_folder_and_caps_at_limit() {
        let out = run(
            &[
                ("a/1.md", ""),
                ("a/2.md", ""),
                ("a/3.md", ""),
                ("b/4.md", ""),
            ],
            json!({"folder": "a", "limit": 2}),
        )
        .await;
        assert_eq!(out["total"], 3);
        assert_eq!(out["truncated"], true);
        assert_eq!(paths(&out), ["a/1.md", "a/2.md"]);
    }

    #[tokio::test]
    async fn returns_an_empty_list_for_an_empty_vault() {
        assert_eq!(
            run(&[], json!({})).await,
            json!({"total": 0, "orphans": []})
        );
    }
}
