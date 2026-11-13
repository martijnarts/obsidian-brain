//! `list_tags`: every tag in the vault with its note count.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::mcp::tool::ToolSpec;
use crate::store::nodes::all_notes;
use crate::util::js::locale_compare;
use crate::vault::tags::{count_tags, sort_tag_counts};

const DESCRIPTION: &str = "List every tag in the vault with the number of notes that carry it. Counts frontmatter `tags`/`tag` and inline `#tags`; tags are returned without the `#`.";

#[derive(Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Sort {
    Count,
    Name,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Default `count` (descending). `name` sorts alphabetically.
    sort: Option<Sort>,
    /// Max tags to return. Default 100.
    #[schemars(range(min = 1))]
    limit: Option<u64>,
    /// Only tags starting with this text, e.g. `project/`.
    prefix: Option<String>,
    /// Default `true`: a note tagged `a/b` also counts toward `a`.
    include_parents: Option<bool>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("list_tags", DESCRIPTION, |ctx, args| async move {
        let cap = args.limit.unwrap_or(100) as usize;
        let prefix = args
            .prefix
            .as_deref()
            .map(|p| p.strip_prefix('#').unwrap_or(p));
        let notes = ctx.db.with(all_notes)?;
        let counts = count_tags(
            notes.iter().map(|n| &n.frontmatter),
            args.include_parents != Some(false),
        );

        let mut tags = sort_tag_counts(counts);
        if let Some(prefix) = prefix.filter(|p| !p.is_empty()) {
            tags.retain(|t| t.tag.starts_with(prefix));
        }
        if args.sort == Some(Sort::Name) {
            tags.sort_by(|a, b| locale_compare(&a.tag, &b.tag));
        }

        Ok(json!({
            "totalTags": tags.len(),
            "truncated": tags.len() > cap,
            "tags": tags.into_iter().take(cap).collect::<Vec<_>>(),
        }))
    })
}
