# Configuration

The vaults come from `--vault <name>=<path>` flags on each command. The README lists the flags. Everything else comes from environment variables. The server reads them at startup; a change takes effect on the next start.

## Storage

| Variable | Default | Effect |
|---|---|---|
| `DATA_DIR` | `$XDG_DATA_HOME/obsidian-brain`, else `~/.local/share/obsidian-brain` | The data directory. Each vault keeps its index in `<DATA_DIR>/<name>/index.db`. |
| `KG_DATA_DIR` | none | Used as `DATA_DIR` when `DATA_DIR` is not set. |
| `TRANSFORMERS_CACHE` | `<DATA_DIR>/models` | Where downloaded models live, in the Hugging Face cache layout. |
| `OBSIDIAN_BRAIN_CONFIG_DIR` | `$XDG_CONFIG_HOME/obsidian-brain`, else `~/.config/obsidian-brain` | The directory of `model-overrides.json`. |

## Embedding model

| Variable | Default | Effect |
|---|---|---|
| `EMBEDDING_PRESET` | `english` | A preset name: `english`, `english-fast`, `english-quality`, `multilingual`, `multilingual-quality` or `multilingual-ollama`. `fastest` and `balanced` still resolve, to `english-fast` and `english`, with a warning. |
| `EMBEDDING_MODEL` | none | A model id. It overrides `EMBEDDING_PRESET`. For `transformers` it is a Hugging Face repository with ONNX weights; for `ollama` it is an Ollama model name. |
| `EMBEDDING_PROVIDER` | the preset's provider, else `transformers` | `transformers` or `ollama`. With a preset of the other provider, the provider wins and the server logs a warning. |
| `OBSIDIAN_BRAIN_EMBEDDER_IDLE_MS` | `0` | Unloads the local model after this many milliseconds without an embed. The next embed loads it again. `0` keeps it loaded. |
| `OBSIDIAN_BRAIN_MAX_CHUNK_TOKENS` | none | Sets the chunk size limit in tokens, in place of the limit the server reads from the model. |
| `OBSIDIAN_BRAIN_MAX_CHUNK_TOKENS_<HASH>` | none | The same for one model. `<HASH>` is the first 32 hex digits of the SHA-256 of the model identifier, such as `Xenova/bge-small-en-v1.5` or `ollama:nomic-embed-text`. It beats the global variable. |
| `ORT_DYLIB_PATH` | set by the Nix wrapper | The ONNX Runtime library to load. The Nix package sets it to the nixpkgs build. |
| `HF_ENDPOINT` | `https://huggingface.co` | The Hugging Face endpoint for model downloads. |

## Ollama

These apply when the provider is `ollama`.

| Variable | Default | Effect |
|---|---|---|
| `OLLAMA_BASE_URL` | `http://localhost:11434` | The Ollama server. |
| `OLLAMA_EMBEDDING_DIM` | none | The vector dimension of the model. When it is set, the server skips the probe embed at startup. It must be a positive number. |
| `OLLAMA_NUM_CTX` | the model's `context_length` from `/api/show`, else 8192 | The `num_ctx` of each embed request. A value below the model's context truncates long chunks. |
| `OBSIDIAN_BRAIN_OLLAMA_AUTO_PULL` | on | `0` never pulls a missing model. |
| `OBSIDIAN_BRAIN_OLLAMA_BYOM_AUTO_PULL` | off | `1` also pulls models outside Ollama's official library. |

## Indexing and the watcher

| Variable | Default | Effect |
|---|---|---|
| `OBSIDIAN_BRAIN_NO_WATCH` | unset | `1` starts no file watcher in `server`. |
| `OBSIDIAN_BRAIN_NO_CATCHUP` | unset | `1` skips the catch-up index at `server` startup. The first index of an empty index still runs. |
| `OBSIDIAN_BRAIN_WATCH_DEBOUNCE_MS` | `3000` | The per-file debounce of the watcher in `server`. The `watch` command takes `--debounce` instead. |

## Tool calls and logs

| Variable | Default | Effect |
|---|---|---|
| `OBSIDIAN_BRAIN_TOOL_TIMEOUT_MS` | `30000`, and `600000` for `reindex` | The time limit of one tool call, in milliseconds. A call that runs longer returns an error. |
| `OBSIDIAN_BRAIN_TOOL_TIMEOUT_MS_<TOOL>` | none | The same for one tool. `<TOOL>` is the tool name in capitals, such as `OBSIDIAN_BRAIN_TOOL_TIMEOUT_MS_REINDEX`. It beats the global variable. |
| `OBSIDIAN_BRAIN_LOG` | `info` | The log filter, in the `EnvFilter` syntax of `tracing`, such as `debug` or `obsidian_brain::pipeline=debug`. Logs go to stderr, because stdout carries the stdio transport. |

## Common combinations

| Setup | Variables |
|---|---|
| English, local model | none |
| Multilingual, local model | `EMBEDDING_PRESET=multilingual` |
| Multilingual through Ollama | `EMBEDDING_PRESET=multilingual-ollama` |
| Any Ollama model | `EMBEDDING_PROVIDER=ollama EMBEDDING_MODEL=nomic-embed-text` |
| Ollama on another host | `EMBEDDING_PROVIDER=ollama OLLAMA_BASE_URL=http://other-host:11434` |
| Any Hugging Face model with ONNX weights | `EMBEDDING_MODEL=Xenova/all-MiniLM-L6-v2` |
