//! The tools that find and read notes: search, read_note, list_notes,
//! find_notes_by_name, grep_vault and query_notes.

mod find_notes_by_name;
mod grep_vault;
mod json_logic;
mod list_notes;
mod query_notes;
mod read_note;
mod search;

use crate::mcp::tool::ToolSpec;

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
