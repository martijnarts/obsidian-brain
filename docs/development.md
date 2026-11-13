# Development

The flake pins the toolchain. The dev shell has cargo, rustc, clippy, rustfmt, rust-analyzer, SQLite and ONNX Runtime, and sets `ORT_DYLIB_PATH` to the nixpkgs ONNX Runtime.

```bash
git clone https://github.com/martijnarts/obsidian-brain.git
cd obsidian-brain
nix develop
cargo run -- server --vault notes="$HOME/path/to/vault"
```

To test a local build in a client, point the client at `target/debug/obsidian-brain` with the arguments `server`, `--vault` and `notes=/absolute/path/to/vault`. Outside the dev shell, the debug binary needs `ORT_DYLIB_PATH` in its environment; the binary of `nix build` sets it on its own.

## Checks

These are the checks CI runs:

```bash
nix develop -c bash -c 'cargo-fmt --all --check && cargo-clippy clippy --all-targets -- -D warnings && cargo test'
nix build .#obsidian-brain
```

`nix build` runs the tests again inside the build.

The tests use a fake embedder, so they need no network. Three tests download a real model from Hugging Face and are ignored by default. Run them with `cargo test -- --ignored`. One of them checks the local embedder against `tests/fixtures/embedder-v4-baseline.json` at a cosine similarity of at least 0.99.

`tests/tool_schemas.rs` checks every tool's name, description and input schema against `tests/fixtures/tools-list.golden.json`. That file is the tool interface the clients rely on. A change to a tool's interface changes that file in the same commit.

## Layout

```
crates/obsidian-brain/
├── data/seed-models.json   # model metadata, compiled into the binary
├── src/
│   ├── cli.rs              # the commands
│   ├── mcp/                # MCP server, tool list, HTTP transport
│   ├── vaults.rs           # the vault set
│   ├── context.rs          # one vault
│   ├── tools/              # the tools, by group
│   ├── pipeline/           # indexer, startup check, watcher
│   ├── embeddings/         # embedders, presets, chunker, metadata
│   ├── store/              # SQLite schema and queries
│   ├── search/             # semantic, full-text and hybrid search
│   ├── graph/              # link graph and centrality
│   ├── vault/              # Markdown files, frontmatter, links
│   └── resolve/            # note name resolution
└── tests/
    ├── fixtures/           # test vault, embedder baseline, tool interface
    ├── cli_e2e.rs          # the binary end to end
    ├── core/               # indexer, search, writer, vault set
    ├── embeddings/         # embedders and metadata
    └── tools/              # each tool
```

[Architecture](architecture.md) describes how the parts work together.
