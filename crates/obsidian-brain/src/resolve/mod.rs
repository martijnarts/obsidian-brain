//! Ported from `src/resolve` of the TypeScript server: turning a note name
//! from a tool argument into indexed node ids.

pub mod name_match;
pub mod single_note;

pub use name_match::resolve_node_name;
pub use single_note::{ResolveError, resolve_single_note};
