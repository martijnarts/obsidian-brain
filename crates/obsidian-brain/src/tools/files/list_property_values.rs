//! `list_property_values`: the distinct values of one frontmatter property.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::args;
use crate::mcp::tool::ToolSpec;
use crate::store::nodes::all_notes;
use crate::util::js::{json_stringify, locale_compare};

const DESCRIPTION: &str = "List the distinct values of one frontmatter property across notes, with how many times each occurs. Each element of a list value counts separately. Also reports how many notes have the key.";

#[derive(Debug, Deserialize, JsonSchema)]
struct Args {
    /// Frontmatter key, e.g. `status`.
    #[serde(deserialize_with = "args::non_empty")]
    #[schemars(length(min = 1))]
    key: String,
    /// Only notes under this folder.
    folder: Option<String>,
    /// Max distinct values to return. Default 100.
    #[serde(default, deserialize_with = "args::opt_positive")]
    #[schemars(range(min = 1))]
    limit: Option<u64>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>(
        "list_property_values",
        DESCRIPTION,
        |ctx, args| async move {
            let key = args.key;
            let cap = args.limit.unwrap_or(100) as usize;
            let folder = args
                .folder
                .as_deref()
                .map(|f| f.trim_end_matches('/'))
                .filter(|f| !f.is_empty())
                .map(|f| format!("{f}/"));

            // Keyed by JSON so the number 5 and the string "5" stay distinct.
            let mut counts: Vec<(String, Value, usize)> = Vec::new();
            let mut notes_with_key = 0;
            for n in ctx.db.with(all_notes)? {
                if folder
                    .as_ref()
                    .is_some_and(|f| !n.id.starts_with(f.as_str()))
                {
                    continue;
                }
                let Some(raw) = n.frontmatter.get(&key) else {
                    continue;
                };
                notes_with_key += 1;
                let values = match raw {
                    Value::Array(items) => items.clone(),
                    other => vec![other.clone()],
                };
                for value in values {
                    let k = json_stringify(&value);
                    match counts.iter_mut().find(|(ck, _, _)| *ck == k) {
                        Some((_, _, count)) => *count += 1,
                        None => counts.push((k, value, 1)),
                    }
                }
            }

            counts.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| locale_compare(&a.0, &b.0)));
            let values: Vec<Value> = counts
                .iter()
                .map(|(_, value, count)| json!({ "value": value, "count": count }))
                .collect();

            Ok(json!({
                "key": key,
                "notesWithKey": notes_with_key,
                "totalDistinct": values.len(),
                "truncated": values.len() > cap,
                "values": values.into_iter().take(cap).collect::<Vec<_>>(),
            }))
        },
    )
}
