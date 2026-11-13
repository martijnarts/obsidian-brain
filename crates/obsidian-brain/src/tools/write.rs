//! The tools that create, edit, link, move and delete notes. Each one
//! queues a background reindex after its write.

mod apply_edit_preview;
mod create_note;
mod create_note_from_template;
mod delete_note;
mod edit_note;
mod link_notes;
mod move_note;

use crate::mcp::tool::ToolSpec;

pub fn specs() -> Vec<ToolSpec> {
    vec![
        create_note::spec(),
        create_note_from_template::spec(),
        edit_note::spec(),
        apply_edit_preview::spec(),
        link_notes::spec(),
        move_note::spec(),
        delete_note::spec(),
    ]
}

/// `path.basename(p, '.md')` on `/`-separated paths.
fn basename_md(p: &str) -> &str {
    let base = p.rsplit('/').next().unwrap_or(p);
    match base.strip_suffix(".md") {
        Some(stem) if !stem.is_empty() => stem,
        _ => base,
    }
}
