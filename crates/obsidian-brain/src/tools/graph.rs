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

use crate::mcp::tool::ToolSpec;

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
