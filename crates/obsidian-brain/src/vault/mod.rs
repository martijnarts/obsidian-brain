//! Ported from `src/vault` of the TypeScript server: the vault layer that
//! reads, parses and edits notes. `writer`, `mover` and `scan` also touch
//! the store; the rest works without it.

pub mod block_id;
pub mod canvas;
pub mod editor;
pub mod frontmatter;
pub mod fuzzy;
pub mod markdown_lines;
pub mod mover;
pub mod note_parts;
pub mod parser;
pub mod scan;
pub mod tags;
pub mod tasks;
pub mod templates;
pub mod vault_path;
pub mod wiki_links;
pub mod writer;
