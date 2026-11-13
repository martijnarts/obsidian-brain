//! The tools that find and read notes: search, read_note, list_notes,
//! find_notes_by_name, grep_vault and query_notes.

mod find_notes_by_name;
mod grep_vault;
mod json_logic;
mod list_notes;
mod query_notes;
mod read_note;
mod search;

use crate::mcp::tool::{InvalidArguments, ToolSpec};

pub fn specs() -> Vec<ToolSpec> {
    vec![
        search::spec(),
        read_note::spec(),
        list_notes::spec(),
        find_notes_by_name::spec(),
        grep_vault::spec(),
        query_notes::spec(),
    ]
}

/// The bounds zod checked before the handler ran. serde only checks the
/// type, so each handler calls this for its bounded integers.
fn check_range(
    field: &str,
    value: Option<u64>,
    min: u64,
    max: Option<u64>,
) -> Result<(), InvalidArguments> {
    let Some(v) = value else { return Ok(()) };
    if v < min {
        return Err(InvalidArguments(format!(
            "{field}: Number must be greater than or equal to {min}"
        )));
    }
    if let Some(max) = max
        && v > max
    {
        return Err(InvalidArguments(format!(
            "{field}: Number must be less than or equal to {max}"
        )));
    }
    Ok(())
}

/// zod's `.min(1)` on a string.
fn check_non_empty(field: &str, value: &str) -> Result<(), InvalidArguments> {
    if value.is_empty() {
        return Err(InvalidArguments(format!(
            "{field}: String must contain at least 1 character(s)"
        )));
    }
    Ok(())
}

/// JS `a.sort()` on strings: by UTF-16 code units.
fn utf16_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}
