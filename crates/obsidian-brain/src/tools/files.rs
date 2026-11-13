//! The file tools: batch and partial reads, file and folder management,
//! attachments, and the vault-wide views of tags, properties and bookmarks.

mod create_attachment;
mod create_folder;
mod delete_folder;
mod file_info;
mod list_attachments;
mod list_bookmarks;
mod list_property_values;
mod list_tags;
mod read_note_part;
mod read_notes;
mod update_properties;
mod vault_overview;

use crate::mcp::tool::ToolSpec;

pub fn specs() -> Vec<ToolSpec> {
    vec![
        vault_overview::spec(),
        list_tags::spec(),
        list_bookmarks::spec(),
        list_property_values::spec(),
        update_properties::spec(),
        read_notes::spec(),
        read_note_part::spec(),
        file_info::spec(),
        create_folder::spec(),
        delete_folder::spec(),
        list_attachments::spec(),
        create_attachment::spec(),
    ]
}
