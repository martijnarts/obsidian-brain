# Architecture

This page describes how the server is built and why. The code is one crate, `crates/obsidian-brain`.

## Modules

| Module | Job |
|---|---|
| `cli` | The `server`, `index`, `watch` and `search` commands, and shutdown. |
| `mcp` | The MCP server on rmcp: the tool list, the `vault` argument, argument validation, time limits and the HTTP transport. |
| `vaults` | Opens, indexes, watches and closes the set of vaults. |
| `context` | One vault: its database, search, writer, index pipeline and embedder state. |
| `tools` | The 41 tools, in five groups: `find`, `write`, `files`, `graph` and `maintenance`. |
| `pipeline` | The indexer, the startup compatibility check and the file watcher. |
| `embeddings` | The local and Ollama embedders, the presets, the chunker, the model metadata chain and the chunk size limits. |
| `store` | The SQLite schema and the queries of each table. |
| `search` | Semantic, full-text and hybrid search. |
| `graph` | The link graph, PageRank, betweenness and paths. |
| `vault` | Reading, parsing, editing and moving Markdown files; frontmatter; wiki links. |
| `resolve` | Turns a note name from a tool argument into a note path. |

Tools reach the database through `search`, `graph`, `vault` and `resolve`, which own the query shapes.

## Several vaults, one server

`server` takes one or more `--vault <name>=<path>` flags. Each vault gets its own context: a database, a search index, a writer, an index pipeline and a watcher. All vaults share one embedder, so the model loads once, and one idle unload frees it for every vault.

The server lists each tool once. Each tool except `list_vaults` has a required `vault` argument whose schema is an enum of the vault names. A call goes to the context of the vault it names. There is no default vault, so a write to the wrong vault needs an explicit wrong name.

At startup the vaults index one at a time in the background, after the MCP handshake. Tools answer during that time.

## Transports

stdio is the default. The client starts the server as a child process, and the server stops with the client. There is no port and no authentication to configure. A watchdog checks once a minute that the parent process is still alive, so a server whose client crashed stops too.

`--transport http` serves streamable HTTP at `/mcp` for a remote client. It is stateless: no tool sends a message the client did not ask for, so each POST gets its own answer. It has no authentication and accepts only loopback `Host` headers and the `--allowed-host` values. Put an authenticating proxy in front of it.

## The index

Each vault has one SQLite file, `<DATA_DIR>/<name>/index.db`. It holds:

- `nodes` and `edges`: the notes and their wiki links. Links to missing notes point at stub nodes.
- `nodes_fts`: an FTS5 index of title and content, with the `porter unicode61` tokenizer.
- `chunks` and `chunks_vec`: the chunks of each note and their vectors, in a sqlite-vec `vec0` table.
- `nodes_vec`: one vector per note, for note-level search.
- `sync`: the modification time of each indexed file.
- `index_metadata`: the model, dimension, provider, model identity, layout version and prefix strategy the index was built with.
- `embedder_capability`: per model, the token limit and the query and document prefixes.
- `failed_chunks`: the chunks that could not be embedded, and why.

One file means one backup and one thing to delete. The vault-relative path is the key of every row. The database runs in WAL mode with a 5-second busy timeout, so two processes can use one index.

The index has a layout version. When the version changes, the server drops the index and builds it again. There are no in-place migrations.

## Indexing

An index pass scans the vault for `.md` files and compares each modification time with `sync`. It parses changed and new files: frontmatter, inline `key:: value` fields, wiki links and embeds. It deletes the nodes of files that are gone. A pass over a vault with no changes costs about one `stat` per file.

The chunker splits a note at headings up to level 4, and splits long sections at paragraphs and sentences. It keeps code fences and `$$` math blocks whole. Each chunk has a SHA-256 hash of its text, so an edit to one section embeds only that section again. A note with no body gets one fallback chunk from its title, tags, scalar frontmatter values and first links, so it stays findable by name.

The chunk size limit is 90% of the model's token limit. When a chunk is too long for the model anyway, the indexer records it in `failed_chunks`, halves the limit for that model and goes on. The limit never drops below 256 tokens, and it resets at the start of each full pass.

## Startup check

At startup, the server compares `index_metadata` with the live embedder. A different model, dimension, provider, Ollama weight digest or prefix strategy deletes the vectors and embeds the vault again in the background. Until that ends, semantic search reports that it is preparing. Full-text search and the other tools work during the rebuild.

## Search

- Semantic search embeds the query and runs a k-nearest-neighbour query on `chunks_vec`. Results group by note, with the best chunk score per note, unless the caller asks for chunks.
- Full-text search runs an FTS5 `MATCH` and orders by `bm25(nodes_fts, 5.0, 1.0)`, so a title match counts five times a body match. Queries with FTS5 operator characters, such as `-` or `:`, are quoted as a phrase.
- Hybrid search, the default, runs both and fuses the ranks with Reciprocal Rank Fusion, with `k = 60`.

sqlite-vec scans every vector. This is fast enough for vaults up to tens of thousands of notes.

## Embeddings

The local embedder downloads `onnx/model_quantized.onnx`, `tokenizer.json` and the model config from Hugging Face with hf-hub, and runs the model with ONNX Runtime on two threads. It mean-pools the last hidden state and normalises the vector. The Nix package links the ONNX Runtime of nixpkgs.

The model metadata comes from a chain:

1. A complete user override in `model-overrides.json`.
2. A row in `embedder_capability`.
3. The bundled seed, `crates/obsidian-brain/data/seed-models.json`, compiled into the binary. It covers 349 models.
4. The loaded model: its dimension, 512 tokens and no prefixes.
5. Safe defaults, with a warning.

A partial override patches whatever the chain resolved. The Ollama embedder reads the context length and the dimension from `/api/show`.

## Writes

Each write tool queues a reindex of the file it wrote. Edits, moves and canvas writes go through a temporary file and a rename. `edit_note` with `dryRun: true` returns a diff and a preview id; `apply_edit_preview` writes it if the file did not change since. Previews expire after 5 minutes. Frontmatter edits change only the lines of the keys they set or remove. `move_note` rewrites the links that point at the moved note.

## The watcher

The watcher uses the notify crate, which uses inotify on Linux and FSEvents on macOS. [Live updates](watching.md) describes the debounce.
