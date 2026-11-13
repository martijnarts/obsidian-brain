# Tool porting brief

Read PORTING.md first. This adds what a tool porter needs.

## Where a tool goes

Each tool group has a module in `crates/obsidian-brain/src/tools/`:
`find.rs`, `write.rs`, `files.rs`, `graph.rs`, `maintenance.rs`. Its
`specs()` returns the group's `ToolSpec`s; `tools::all()` sorts them into
`tools::ORDER`. Put one tool per file under the group's directory, for
example `tools/find/search.rs` with `mod search;` in `tools/find.rs`. Edit
only your group's files.

## How a tool is written

```rust
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Args {
    /// Natural-language query or keyword phrase.
    query: String,
    /// Default `hybrid`. Semantic-only queries chunk vectors; fulltext-only queries FTS5.
    mode: Option<Mode>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("search", DESCRIPTION, |ctx, args| async move { ... })
}
```

- The handler returns `anyhow::Result<serde_json::Value>`. A
  `Value::String` is sent as plain text; anything else is pretty-printed
  JSON. An `Err` becomes `Error: <message>` with `isError`. These match the
  TS `registerTool` in `src/tools/register.ts`.
- Field doc comments become schema descriptions. A doc comment on the
  struct itself is dropped. Use `#[schemars(description = "...")]` when the
  exact text is awkward as a doc comment.
- Do not use `deny_unknown_fields` if zod would have stripped unknown keys;
  zod objects strip unknown keys by default, so in general leave it off.
- `crates/obsidian-brain/tests/tool_schemas.rs` diffs every ported tool's
  name, description and input schema against the TS output in
  `conformance/tools-list.golden.json`. It must pass for every tool you
  port. Use schemars attributes (`range`, `length`, `extend`) to get the
  same constraints. If a difference is a pure spelling difference that
  means the same, teach `normalize` in that test about it, narrowly, and
  say so in your report.
- Read the TS tool file in `src/tools/` and port its behaviour, output
  shape, field names, defaults and error messages exactly. Port its tests
  from `test/tools/` as Rust integration tests in
  `crates/obsidian-brain/tests/tools/<group>/`, or as unit tests.

## The API you build on

- Context: `crate::context::VaultContext` (fields `config`, `db`, `embedder`,
  `search`, `writer`, `pipeline`). See its doc comments.
- Store: `store::Db` (`with`, `with_tx`, `run` on a blocking thread) and
  the functions in `store::*` on `&rusqlite::Connection`. Never hold the
  lock across `.await`.
- Vault layer: `vault::*` (`editor`, `frontmatter`, `parser`, `scan`,
  `writer`, `mover`, `tasks`, `tags`, `note_parts`, `block_id`, `canvas`,
  `templates`, `vault_path`, `wiki_links`). Every path a client gives goes
  through `vault::vault_path::resolve_vault_path` or `resolve_folder`.
- Graph: `graph::*`. Name resolution: `resolve::*`.
- Search: `search::Search` (`hybrid`, `semantic`, `semantic_chunks`,
  `fulltext`).
- Tool support: `tools::support::{hints, preparing, background_reindex,
  preview_store, edit_buffer}`. Write tools call
  `background_reindex::run_background_reindex(&ctx)` after the write, as
  the TS did.
- JS semantics helpers: `util::js` (UTF-16 lengths and slices, JS trim,
  `JSON.stringify`, `String(v)`, number formatting). User regexes:
  `util::safe_regex::compile_user_regex`.
- Test helpers: `tests/core/support.rs` has a `FakeEmbedder`. A lean
  context is
  `VaultContext::from_parts(VaultConfig::new(name, vault, data), Db::open_in_memory()?, share_embedder(Arc::new(fake)), true)`.

## Frontmatter

Marts decided that writes edit frontmatter in place: only the lines of the
keys a tool changes may change. Use `vault::frontmatter::edit_frontmatter`
or `vault::editor::update_frontmatter`, never a full re-dump, except where
the TS created a new note from scratch (`frontmatter::stringify`).
