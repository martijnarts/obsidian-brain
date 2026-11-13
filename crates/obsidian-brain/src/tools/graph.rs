//! Graph, index, canvas and task tools: `find_connections`,
//! `find_path_between`, `rank_notes`, `reindex`, `index_status`,
//! `read_canvas`, `edit_canvas`, `list_tasks`, `set_task_status` and
//! `ensure_block_id`.

mod edit_canvas;
mod ensure_block_id;
mod find_connections;
mod find_path_between;
mod index_status;
mod list_tasks;
mod patch;
mod rank_notes;
mod read_canvas;
mod reindex;
mod set_task_status;

use serde::de::{Deserialize, Deserializer, Error as _};
use serde_json::Value;

use crate::mcp::tool::ToolSpec;
use crate::util::js;

pub fn specs() -> Vec<ToolSpec> {
    vec![
        find_connections::spec(),
        find_path_between::spec(),
        rank_notes::spec(),
        read_canvas::spec(),
        edit_canvas::spec(),
        reindex::spec(),
        index_status::spec(),
        list_tasks::spec(),
        set_task_status::spec(),
        ensure_block_id::spec(),
    ]
}

/// zod's `.int().positive()`: an integer of at least 1.
fn positive<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    let n = u64::deserialize(d)?;
    if n == 0 {
        return Err(D::Error::custom("expected a positive integer, got 0"));
    }
    Ok(n)
}

/// [`positive`] for an optional field; use with `#[serde(default)]`.
fn opt_positive<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    Option::<u64>::deserialize(d)?
        .map(|n| {
            if n == 0 {
                Err(D::Error::custom("expected a positive integer, got 0"))
            } else {
                Ok(n)
            }
        })
        .transpose()
}

/// zod's `.min(1)` on a string.
fn non_empty<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let s = String::deserialize(d)?;
    if s.is_empty() {
        return Err(D::Error::custom(
            "expected a string of at least 1 character",
        ));
    }
    Ok(s)
}

/// Integral floats as JSON integers, as `JSON.stringify` prints them.
fn js_numbers(value: Value) -> Value {
    match value {
        Value::Number(n) if n.is_f64() => js::number_value(js::as_f64(&n)),
        Value::Array(items) => Value::Array(items.into_iter().map(js_numbers).collect()),
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, js_numbers(v))).collect())
        }
        other => other,
    }
}
