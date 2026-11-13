# Porting brief

This repository is being ported from TypeScript (`src/`, `test/`) to Rust
(`crates/obsidian-brain/`). The TypeScript stays until the Rust server
reaches parity, and then it is deleted. Read this before you change any
Rust.

## Rules

- Port behaviour faithfully, function by function. A TS function
  `fooBar` in `src/x/foo-bar.ts` becomes `foo_bar` in
  `crates/obsidian-brain/src/x/foo_bar.rs`. Keep the same inputs, outputs,
  error messages and edge cases. Tool output text and JSON field names
  must stay identical, because MCP clients read them.
- Port the matching vitest tests in `test/` to Rust tests. Skip tests that
  only exercise mocks of a TypeScript library, and say which in your
  report.
- Use the shared types in `src/types.rs`. Do not change them. A type you
  need that is not there belongs in your own module.
- Touch only your own module directories, plus `Cargo.toml` when you need a
  dependency. Other agents work on other modules at the same time.
- Comments: match the density of the TS. Write them in plain, short,
  active sentences. Drop version archaeology ("v1.7.5 fixed ..."), issue
  references and history. Say what the code does and why.
- JS semantics to watch:
  - JS string indices are UTF-16 code units. Rust uses bytes. Wherever an
    index or length leaves the module (a column, an offset, a slice
    length, a character limit), compute it in the unit the TS used.
  - JS `RegExp` features (lookbehind, `u` flag, named groups): use
    `fancy-regex` where `regex` cannot express the pattern.
  - JSON objects keep insertion order (`serde_json` has `preserve_order`).
  - YAML: gray-matter parsed with js-yaml. A bare date like `2024-01-01`
    became a JS `Date`, which JSON shows as `"2024-01-01T00:00:00.000Z"`.
- Keep `cargo clippy --all-targets` and `cargo fmt --check` clean. Run
  everything through `nix develop -c ...` from the worktree root.
- Commit on your own branch with plain commit messages. Never push and
  never open a pull request.
- Never call OpenRouter or any paid API.

## Report

End with: what you ported (file by file), the public API other modules
call (signatures), the tests you ported and skipped, every behaviour you
changed on purpose, and anything left undone.
