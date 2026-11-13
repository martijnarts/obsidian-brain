//! What the tool handlers share: the `next_actions` hints, the
//! embedder-preparing response, the background reindex, the unified diff
//! of a preview, and the process-wide edit preview and failed-edit stores.

pub mod background_reindex;
pub mod edit_buffer;
pub mod hints;
pub mod patch;
pub mod preparing;
pub mod preview_store;

pub use background_reindex::run_background_reindex;
pub use edit_buffer::{BufferEntry, EDIT_BUFFER, EditBuffer, PushOutcome};
pub use hints::{
    ContextualResult, FindConnectionsHintNeighbor, NextAction, ReadNoteHintInput, SearchHintHit,
    StateTokens, ToolContext, compute_find_connections_hints, compute_read_note_hints,
    compute_search_hints, is_contextual_result,
};
pub use preparing::{PreparingResponse, PreparingStatus, describe_embedder_preparing};
pub use preview_store::{PREVIEW_STORE, PendingEdit, PreviewStore};
