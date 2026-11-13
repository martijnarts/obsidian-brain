//! Every vault tool, in the order `tools/list` shows them, and the
//! infrastructure the tools share in [`support`].
//!
//! Each group module returns its tools from `specs()`. `all()` sorts them
//! into [`ORDER`], the order of the TypeScript server.

mod files;
mod find;
mod graph;
mod maintenance;
pub mod support;
mod write;

use crate::mcp::tool::ToolSpec;

/// The tool order of `tools/list`, after `list_vaults`.
pub const ORDER: [&str; 40] = [
    "search",
    "read_note",
    "list_notes",
    "find_notes_by_name",
    "grep_vault",
    "query_notes",
    "find_connections",
    "find_path_between",
    "rank_notes",
    "create_note",
    "create_note_from_template",
    "edit_note",
    "apply_edit_preview",
    "link_notes",
    "move_note",
    "delete_note",
    "read_canvas",
    "edit_canvas",
    "reindex",
    "index_status",
    "vault_overview",
    "list_tags",
    "list_bookmarks",
    "list_property_values",
    "update_properties",
    "read_notes",
    "read_note_part",
    "file_info",
    "create_folder",
    "delete_folder",
    "list_attachments",
    "create_attachment",
    "list_tasks",
    "set_task_status",
    "ensure_block_id",
    "find_broken_links",
    "find_orphaned_notes",
    "search_and_replace",
    "rename_tag",
    "rename_heading",
];

pub fn all() -> Vec<ToolSpec> {
    let mut tools: Vec<ToolSpec> = [
        find::specs(),
        write::specs(),
        files::specs(),
        graph::specs(),
        maintenance::specs(),
    ]
    .into_iter()
    .flatten()
    .collect();
    let rank = |t: &ToolSpec| ORDER.iter().position(|n| *n == t.name).unwrap_or(ORDER.len());
    tools.sort_by_key(rank);
    tools
}
