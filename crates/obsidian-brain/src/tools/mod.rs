//! Every vault tool, in the order `tools/list` shows them, and the
//! infrastructure the tools share in [`support`].

pub mod support;

use crate::mcp::tool::ToolSpec;

pub fn all() -> Vec<ToolSpec> {
    vec![]
}
