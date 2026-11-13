# obsidian-brain

obsidian-brain is an MCP server for Obsidian vaults. It gives Claude and other MCP clients semantic search, a link graph and note editing. One process serves one or more vaults, over stdio for a local client or over HTTP for a remote one. It reads the Markdown files on disk, so Obsidian does not need to run. Embeddings run on your machine, so note content stays local.

This is [Marts's fork](https://github.com/martijnarts/obsidian-brain) of [sweir1/obsidian-brain](https://github.com/sweir1/obsidian-brain) by sweir1. Version 2 is a rewrite in Rust. It is not the `obsidian-brain` package on npm: that package is the upstream TypeScript server and does not have the changes of this fork.

## Build and run

The flake builds one binary. It links SQLite and ONNX Runtime from nixpkgs, so it needs no Node and no native modules.

Run it straight from GitHub:

```bash
nix run github:martijnarts/obsidian-brain -- server --vault notes=/path/to/vault
```

Install it in your profile:

```bash
nix profile add github:martijnarts/obsidian-brain
obsidian-brain server --vault notes=/path/to/vault
```

Build it from a clone:

```bash
nix build .#obsidian-brain
./result/bin/obsidian-brain --help
```

The flake builds for `aarch64-linux`, `x86_64-linux` and `aarch64-darwin`. [Development](docs/development.md) covers the dev shell and the tests.

To connect a client, give it the binary as the command and `server --vault name=path` as the arguments. [Install in your MCP client](docs/install-clients.md) has the config for each client.

On the first start the server downloads the embedding model (about 34 MB for the default preset) and indexes each vault in the background. Tools answer during that time. Semantic search reports that it is preparing until the model is ready.

## Commands

`obsidian-brain` has four subcommands. Each takes its vaults as `--vault <name>=<path>`. A name has letters, digits, `-` and `_`.

### `server`

Starts the MCP server.

| Flag | Default | Effect |
|---|---|---|
| `--vault <name=path>` | required | A vault to serve. Repeat it for more vaults. |
| `--transport <stdio\|http>` | `stdio` | `stdio` for a client that starts the server. `http` for a long-running server. |
| `--listen <addr:port>` | `127.0.0.1:8080` | The address to listen on. Only valid with `--transport http`. Port `0` picks a free port. |
| `--allowed-host <host>` | none | A `Host` header to accept with `--transport http`, in addition to `localhost`, `127.0.0.1` and `::1`. Repeat it for more hosts. |

Every tool except `list_vaults` takes a required `vault` argument. Its value is one of the `--vault` names. There is no default vault. Each vault has its own index, link graph and file watcher. All vaults share one embedding model. To give a client only some vaults, run a second server with only those vaults.

With `--transport stdio`, the server stops when the client closes the connection, on SIGINT or SIGTERM, and when the process that started it exits.

With `--transport http`, the server answers MCP requests at `/mcp`. It is stateless: each POST gets its own answer. It has no authentication. Bind it to loopback and put an authenticating proxy in front of it. The server refuses a request whose `Host` header is not loopback or an `--allowed-host`. A proxy that rewrites `Host` to the backend address needs no `--allowed-host`. A proxy that passes the public host name through needs that name as an `--allowed-host`.

```bash
DATA_DIR=/var/lib/obsidian-brain obsidian-brain server \
  --transport http --listen 127.0.0.1:8091 \
  --vault personal=/srv/vaults/personal \
  --vault dnd=/srv/vaults/dnd
```

### `index`

Scans each vault and updates its index, one vault at a time. It prints the statistics as JSON, keyed by vault name. The index is incremental: it only parses and embeds files whose modification time changed.

| Flag | Effect |
|---|---|
| `--vault <name=path>` | A vault to index. Repeat it for more vaults. |
| `--drop` | Deletes all embeddings and sync state before indexing. A model change does this on its own. |

### `watch`

Keeps each vault index current: it reindexes a note after it changes. It stops on SIGINT, SIGTERM and when its standard input closes.

| Flag | Default | Effect |
|---|---|---|
| `--vault <name=path>` | required | A vault to watch. Repeat it for more vaults. |
| `--debounce <ms>` | `3000` | The per-file reindex debounce. |

### `search`

Searches one vault and prints the results as JSON.

| Flag | Default | Effect |
|---|---|---|
| `<query>` | required | The search text. |
| `--vault <name=path>` | required | The vault to search. Give exactly one. |
| `-l, --limit <n>` | `10` | The maximum number of results. |
| `-m, --mode <mode>` | `hybrid` | `hybrid`, `semantic` or `fulltext`. |

## Tools

The server has 41 tools.

- Vaults: `list_vaults`
- Find: `search`, `list_notes`, `read_note`, `find_notes_by_name`, `grep_vault`, `query_notes`
- Files: `read_notes`, `read_note_part`, `file_info`, `create_folder`, `delete_folder`, `list_attachments`, `create_attachment`
- Write: `create_note`, `create_note_from_template`, `edit_note`, `apply_edit_preview`, `link_notes`, `move_note`, `delete_note`
- Properties: `list_property_values`, `update_properties`
- Structure: `vault_overview`, `list_tags`, `list_bookmarks`
- Tasks and blocks: `list_tasks`, `set_task_status`, `ensure_block_id`
- Canvas: `read_canvas`, `edit_canvas`
- Graph: `find_connections`, `find_path_between`, `rank_notes`
- Maintenance: `reindex`, `index_status`, `find_broken_links`, `find_orphaned_notes`, `search_and_replace`, `rename_tag`, `rename_heading`

[Tool reference](docs/tools.md) lists the arguments of each tool.

## Configuration

The vaults come from `--vault` flags. Everything else comes from environment variables, and each one has a default. [Configuration](docs/configuration.md) lists them all. The ones you are most likely to set:

| Variable | Default | Effect |
|---|---|---|
| `DATA_DIR` | `$XDG_DATA_HOME/obsidian-brain`, else `~/.local/share/obsidian-brain` | Where the indexes and the model cache live. |
| `EMBEDDING_PRESET` | `english` | The embedding model preset. |
| `EMBEDDING_PROVIDER` | `transformers` | `transformers` runs the model in the server. `ollama` uses an Ollama server. |
| `EMBEDDING_MODEL` | none | A model id. It overrides the preset. |
| `OBSIDIAN_BRAIN_EMBEDDER_IDLE_MS` | `0` | Unloads the model after this many idle milliseconds. `0` keeps it loaded. |
| `OBSIDIAN_BRAIN_LOG` | `info` | The log filter, in `tracing` `EnvFilter` syntax. Logs go to stderr. |

## Embedding models

`EMBEDDING_PRESET` picks a model by name:

| Preset | Model | Provider |
|---|---|---|
| `english` (default) | `Xenova/bge-small-en-v1.5` | transformers |
| `english-fast` | `MongoDB/mdbr-leaf-ir` | transformers |
| `english-quality` | `Xenova/bge-base-en-v1.5` | transformers |
| `multilingual` | `Xenova/multilingual-e5-small` | transformers |
| `multilingual-quality` | `Xenova/multilingual-e5-base` | transformers |
| `multilingual-ollama` | `qwen3-embedding:0.6b` | ollama |

The `transformers` provider downloads the model from Hugging Face and runs it with ONNX Runtime. To use Ollama, set `EMBEDDING_PRESET=multilingual-ollama`, or set `EMBEDDING_PROVIDER=ollama` and `EMBEDDING_MODEL` to an Ollama model. The server pulls a missing model from Ollama's official library on its own. [Embedding models](docs/embeddings.md) has the details.

When the model changes, the server deletes the old vectors and embeds the vaults again on the next start.

## Where data lives

- Index: `<DATA_DIR>/<name>/index.db`, one SQLite file per vault name. It holds the notes, links, full-text index, vectors and sync state. You can delete it at any time; the server builds it again from the vault.
- Models: `<DATA_DIR>/models`, in the Hugging Face cache layout. `TRANSFORMERS_CACHE` moves it.
- Model overrides: `~/.config/obsidian-brain/model-overrides.json`. [Embedding models](docs/embeddings.md#model-overrides) describes the file.

A server and a `watch` process share an index when they use the same `DATA_DIR` and the same vault name.

## More

- [Architecture](docs/architecture.md): how indexing, search and the vault set work.
- [Live updates](docs/watching.md): the file watcher and its debounce.
- [Running as a service](docs/systemd.md) on Linux, and [on macOS](docs/launchd.md).
- [Troubleshooting](docs/troubleshooting.md)
- [Changelog](docs/CHANGELOG.md)

## Credits

sweir1 wrote [obsidian-brain](https://github.com/sweir1/obsidian-brain), the TypeScript server this rewrite follows. That project drew on [obra/knowledge-graph](https://github.com/obra/knowledge-graph) and [aaronsb/obsidian-mcp-plugin](https://github.com/aaronsb/obsidian-mcp-plugin). This version uses [sqlite-vec](https://github.com/asg017/sqlite-vec) for vector search, [ONNX Runtime](https://onnxruntime.ai) for local embeddings and [rmcp](https://github.com/modelcontextprotocol/rust-sdk) for MCP.

## License

[Apache License 2.0](./LICENSE). Copyright 2026 sweir1.
