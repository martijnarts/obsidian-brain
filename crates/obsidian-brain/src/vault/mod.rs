//! Ported from `src/vault` of the TypeScript server: the pure vault layer
//! that reads, parses and edits notes without the store.

pub mod block_id;
pub mod canvas;
pub mod editor;
pub mod frontmatter;
pub mod fuzzy;
pub mod markdown_lines;
pub mod note_parts;
pub mod parser;
pub mod tags;
pub mod tasks;
pub mod templates;
pub mod vault_path;
pub mod wiki_links;
