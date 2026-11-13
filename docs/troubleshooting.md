# Troubleshooting

## Reading the logs

The server logs to stderr. A stdio client writes that stream to its MCP log, for example `~/Library/Logs/Claude/mcp-server-obsidian-brain.log` for Claude Desktop on macOS. For more detail, set `OBSIDIAN_BRAIN_LOG=debug` in the `env` block of the client config. The value takes the `EnvFilter` syntax of `tracing`, so `obsidian_brain::pipeline=debug` limits the detail to the index pipeline.

## The server does not start: missing `--vault`

`obsidian-brain server` needs at least one `--vault <name>=<path>`. Without one, it exits with `the following required arguments were not provided: --vault <VAULTS>`. Add `"--vault", "notes=/absolute/path/to/your/vault"` to the `args` of the client config. A name has letters, digits, `-` and `_`.

## The client shows no tools

The server stopped at startup. Check the client's MCP log for the last `obsidian-brain` line. Common causes:

- The command path is wrong. Desktop apps do not always see the `PATH` of your shell; use the absolute path from `command -v obsidian-brain`.
- A vault path does not exist.
- An environment variable has a bad value, such as an unknown `EMBEDDING_PRESET`. The error names the valid values.

## The first start is slow

On the first start the server downloads the embedding model into `<DATA_DIR>/models` (about 34 MB for the default preset, up to 279 MB for `multilingual-quality`) and indexes every vault. Tools answer during that time, and semantic search reports that it is preparing. Later starts reuse the model and index only changed files.

If the download fails, the next start tries again. When the model files are damaged, the server deletes that model's cache directory and downloads it once more on its own. To start clean by hand, delete `<DATA_DIR>/models`.

## The index misses an edit

The watcher in `server` reindexes a note about 5 seconds after its last save. If an edit does not appear:

- `OBSIDIAN_BRAIN_NO_WATCH=1` may be set in the client config, the shell, a systemd `Environment=` line or a launchd `EnvironmentVariables` dictionary.
- The vault may be on SMB, NFS or a sync folder where inotify and FSEvents miss changes.
- The file may not end in `.md`, or it may be under `.obsidian`, `.trash`, `.git`, `node_modules` or `attachments`.

The `reindex` tool or `obsidian-brain index --vault notes=/path/to/vault` updates the index at once. For a vault the watcher cannot see, run `index` on a timer, as [Running as a service](systemd.md) shows.

## Watcher not firing

At startup, the server logs `watching <path> for changes` for each vault. If that line is missing, look for `OBSIDIAN_BRAIN_NO_WATCH`. If it is present, the file system does not report changes; see the previous section.

## "No note found matching X" or "Multiple notes match X"

A tool that takes a note name could not resolve it. "No note found" means no note matches. "Multiple notes match" lists the candidates. Pass the vault-relative path, such as `Concepts/Widget Theory.md`, in place of the short name. The `id` of a `search` result is such a path.

## Search results look wrong

The default `search` mode is `hybrid`: it combines semantic and full-text ranks. For a phrase you know is in the vault, pass `mode: "fulltext"`. For a question about a concept, pass `mode: "semantic"`.

For better semantic results, switch to a larger model, such as `EMBEDDING_PRESET=english-quality`. The server embeds the vault again on the next start; no `--drop` is needed.

## Semantic search says it is preparing

The embedder is loading, or the server is embedding the vault again after a model change. Full-text search and the other tools work in the meantime. `index_status` shows the progress.

## `index_status` reports missing embeddings

`notesMissingEmbeddings` counts notes whose chunks failed to embed. `notesNoEmbeddableContent` counts notes with nothing to embed: no title, no frontmatter and no body. `index_status` also lists the chunks skipped in the last pass and why. A chunk that is too long for the model is skipped, and the indexer lowers the chunk size limit for later chunks.

## The index is damaged

Errors such as `database disk image is malformed` or `no such table` during an index pass mean a damaged index. Stop the server and delete the index of that vault:

```bash
rm "$DATA_DIR/<name>/index.db"*
```

The default `DATA_DIR` is `~/.local/share/obsidian-brain`. The next start builds the index again from the vault; the notes are not touched.

## An index of version 1

Version 2 keeps its index in `<DATA_DIR>/<name>/index.db` and its models in the Hugging Face cache layout. It does not read the `kg.db` files or the model cache of version 1. It builds a new index and downloads the model on the first start. You can delete the old `kg.db` files.

## Tool calls time out

A tool call that runs longer than its time limit returns an error that names the tool and the limit. The limit is 30 seconds, and 10 minutes for `reindex`. `OBSIDIAN_BRAIN_TOOL_TIMEOUT_MS` changes it for every tool; `OBSIDIAN_BRAIN_TOOL_TIMEOUT_MS_<TOOL>` changes it for one.

If a call hangs in the client with no error, check the server log for the request. A request that never appears in the server log did not reach the server.

## Several clients on one vault

Each stdio client starts its own server. Two servers with the same `DATA_DIR` and the same vault name share one index file. SQLite runs in WAL mode with a 5-second busy timeout, so two writers take turns. Each server runs a catch-up index at startup and a watcher, and each loads its own copy of the model. Set `OBSIDIAN_BRAIN_NO_CATCHUP=1` and `OBSIDIAN_BRAIN_NO_WATCH=1` for the client you use less. To share one process, run `server --transport http` and connect both clients to it.

## `apply_edit_preview` fails

`File "X" has changed since the preview was generated` means the note changed between `edit_note` with `dryRun: true` and the apply. `Preview "X" not found or expired` means more than 5 minutes passed, or the server restarted. In both cases, run the dry run again and apply the new preview id.

## `edit_note` with `replace_window` returns `NoMatch`

The `search` text is not in the note. Retry with `from_buffer: true`: the server keeps the failed `content` and `search` for 30 minutes and retries them with `fuzzy: true` and `fuzzyThreshold: 0.5`. Content over 512 KB is not kept. You can also pass `fuzzy: true` and a lower `fuzzyThreshold` yourself.

## An Ollama model is not pulled

For a model outside Ollama's official library, such as `user/custom-model`, the server does not pull it and says so. Run `ollama pull user/custom-model`, or set `OBSIDIAN_BRAIN_OLLAMA_BYOM_AUTO_PULL=1`. With `OBSIDIAN_BRAIN_OLLAMA_AUTO_PULL=0`, the server pulls no model at all.

If the Ollama server is not reachable, the error asks whether Ollama runs at `OLLAMA_BASE_URL` with the model pulled.

## macOS: the server cannot read the vault

A vault under `~/Documents`, `~/Desktop` or iCloud Drive needs Full Disk Access. A stdio server gets the access of the client that starts it, so grant Full Disk Access to the client app in System Settings, Privacy and Security, and restart it.

