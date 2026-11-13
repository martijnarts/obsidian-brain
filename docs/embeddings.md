# Embedding models

Semantic search turns each chunk of a note into a vector and finds the chunks closest to the query. The model that makes the vectors is set by environment variables. [Configuration](configuration.md) lists them all.

## Presets

`EMBEDDING_PRESET` picks a model by name. The default is `english`.

| Preset | Model | Provider | Dimension | Download | Languages |
|---|---|---|---|---|---|
| `english` | `Xenova/bge-small-en-v1.5` | transformers | 384 | about 34 MB | English |
| `english-fast` | `MongoDB/mdbr-leaf-ir` | transformers | 768 | about 22 MB | English |
| `english-quality` | `Xenova/bge-base-en-v1.5` | transformers | 768 | about 110 MB | English |
| `multilingual` | `Xenova/multilingual-e5-small` | transformers | 384 | about 135 MB | 94 languages |
| `multilingual-quality` | `Xenova/multilingual-e5-base` | transformers | 768 | about 279 MB | 94 languages |
| `multilingual-ollama` | `qwen3-embedding:0.6b` | ollama | 1024 | about 600 MB | over 100 languages |

All six models are asymmetric: queries and documents get different prefixes. The server applies the prefixes from the bundled seed, so you do not set them.

The server logs a warning for `multilingual-quality`: the model has a known problem with `token_type_ids` on inputs over about 400 words. Chunks that fail are recorded in `failed_chunks`, and `index_status` reports them.

Example client config with a preset:

```json
{
  "mcpServers": {
    "obsidian-brain": {
      "command": "/home/you/.nix-profile/bin/obsidian-brain",
      "args": ["server", "--vault", "notes=/absolute/path/to/your/vault"],
      "env": { "EMBEDDING_PRESET": "multilingual" }
    }
  }
}
```

## Other models

`EMBEDDING_MODEL` sets a model id directly, and it overrides the preset.

With the `transformers` provider, the id is a Hugging Face repository. The repository must have `onnx/model_quantized.onnx` and `tokenizer.json`. The `Xenova/` and `onnx-community/` repositories usually have them. A repository without ONNX weights fails to load with an error that names the `Xenova/` port to try.

With the `ollama` provider, the id is an Ollama model name, such as `nomic-embed-text` or `mxbai-embed-large`.

## Ollama

Set `EMBEDDING_PRESET=multilingual-ollama`, or set `EMBEDDING_PROVIDER=ollama` with an `EMBEDDING_MODEL`. Without a model, the Ollama provider uses `nomic-embed-text`.

```bash
EMBEDDING_PROVIDER=ollama EMBEDDING_MODEL=nomic-embed-text \
  obsidian-brain server --vault notes=/path/to/vault
```

The server talks to `OLLAMA_BASE_URL`, by default `http://localhost:11434`. It reads the model's context length and dimension from `/api/show`. `OLLAMA_EMBEDDING_DIM` sets the dimension and skips the probe embed. `OLLAMA_NUM_CTX` sets the context length of each request; leave it unset unless the model reports a wrong one.

When the model is not in Ollama, the server pulls it:

| Model | Pulled? |
|---|---|
| The model of a preset | yes |
| A name in Ollama's official library, such as `nomic-embed-text` or `library/bge-m3` | yes |
| A name in another namespace, such as `user/custom-model` | only with `OBSIDIAN_BRAIN_OLLAMA_BYOM_AUTO_PULL=1` |

`OBSIDIAN_BRAIN_OLLAMA_AUTO_PULL=0` turns pulling off for every model. The server then fails to start the embedder and names the `ollama pull` command to run.

## Changing the model

Change the variable and restart the server. At startup the server sees that the model, dimension or provider differs from the one the index was built with. It deletes the old vectors and embeds the vault again in the background. The MCP handshake and the other tools do not wait for this. Semantic search reports that it is preparing until the new vectors are ready. `index_status` shows the progress.

The same happens when an Ollama model's weights change under the same tag, and when the prefixes of the model change.

## Model overrides

`model-overrides.json` in the config directory (`~/.config/obsidian-brain/` by default, or `OBSIDIAN_BRAIN_CONFIG_DIR`) patches the metadata of a model. Each field is optional:

```json
{
  "$version": 1,
  "models": {
    "MongoDB/mdbr-leaf-ir": { "maxTokens": 1024 },
    "my-org/my-embedder": {
      "maxTokens": 4096,
      "queryPrefix": "Query: ",
      "documentPrefix": "Document: "
    }
  }
}
```

An entry with all three fields is used as it is. A partial entry patches the metadata the server resolved from its cache, the bundled seed or the loaded model. `null` for a prefix clears it. A change of prefix embeds the vault again on the next start; a change of `maxTokens` applies on the next index pass. A bad file or field is skipped with a warning.

## Chunk sizes

The chunk size limit is 90% of the model's token limit. The local embedder reads the limit from the tokenizer; Ollama reports it in `/api/show`. When a chunk is still too long, the indexer halves the limit for that model, down to 256 tokens, and records the chunk in `failed_chunks`. `OBSIDIAN_BRAIN_MAX_CHUNK_TOKENS` sets the limit by hand.

## Memory

The local model stays loaded by default. `OBSIDIAN_BRAIN_EMBEDDER_IDLE_MS` unloads it after that many idle milliseconds. The next embed loads it again, so the first search after an idle period waits for the load.
