---
title: Jan integration
description: Wire obsidian-brain into Jan. Covers STDIO setup, the HTTP transport bug in rmcp, and Jan-specific gotchas.
---

# Wiring obsidian-brain into Jan

## TL;DR

Point Jan at the stdio server directly. It works. Don't use HTTP in Jan 0.7.x.

## Two-minute setup (UI path)

First install the package once — you only need to do this step if you haven't already:

```bash
npm install -g obsidian-brain
which obsidian-brain    # note the absolute path — you'll need it
```

Then in [Jan](https://jan.ai):

1. Navigate to **Settings -> MCP Servers -> + Add**.
2. Fill in the fields:
   - **Name**: `obsidian-brain`
   - **Transport**: **STDIO (local process)**
   - **Command**: the absolute path from `which obsidian-brain` (typically `/opt/homebrew/bin/obsidian-brain` on macOS Homebrew, `/usr/bin/obsidian-brain` or an nvm-scoped path on Linux). Use an absolute path, **not** a bare `obsidian-brain`. Jan spawns subprocesses with a minimal `PATH` that usually doesn't include your shell's install.
   - **Arguments**: `server`, `--vault`, `notes=/absolute/path/to/your/vault` (three separate arg entries)
3. Save and enable the server. Jan will spawn the process and send `initialize` followed by `tools/list`. First boot auto-indexes the vault (30–60 s while the default embedding model downloads — ~34 MB). Once the index is built you should see the 18 obsidian-brain tools appear in the MCP panel.

### Alternative: npx (no global install)

If you'd rather not install globally, point Jan at `npx` directly:

- **Command**: absolute path to `npx` (e.g. `/opt/homebrew/bin/npx`)
- **Arguments**: `-y`, `obsidian-brain`, `server`, `--vault`, `notes=/absolute/path/to/your/vault` (five separate arg entries)

npx will fetch the package from npm on first launch and cache it locally; subsequent launches are fast.

## Config file path

If you'd rather edit JSON than click through the UI, Jan keeps MCP config here:

- **macOS**: `~/Library/Application Support/Jan/data/mcp_config.json`
- **Linux**: `~/.local/share/Jan/data/mcp_config.json`

Jan places server entries at the top level of its MCP config — there is no `mcpServers` wrapper, unlike Claude Desktop.

The JSON shape is roughly:

```json
{
  "obsidian-brain": {
    "command": "npx",
    "args": ["-y", "obsidian-brain@latest", "server", "--vault", "notes=/absolute/path/to/your/vault"],
    "env": {
      "EMBEDDING_PRESET": "multilingual"
    }
  }
}
```

**Caveat**: Jan's config-file schema is not publicly pinned and has shifted across point releases. Jan prefers the UI as the authoritative path for adding MCP servers. If the shape above stops working after a Jan upgrade, fall back to the UI flow and let Jan regenerate the file.

## Verify it works

Start a new chat in Jan and ask:

> List my obsidian-brain tools.

The assistant should respond naming all 41 tools:

- `list_vaults`
- `search`
- `list_notes`
- `read_note`
- `find_notes_by_name`
- `grep_vault`
- `query_notes`
- `read_notes`
- `read_note_part`
- `file_info`
- `create_folder`
- `delete_folder`
- `list_attachments`
- `create_attachment`
- `create_note`
- `create_note_from_template`
- `edit_note`
- `apply_edit_preview`
- `link_notes`
- `move_note`
- `delete_note`
- `list_property_values`
- `update_properties`
- `vault_overview`
- `list_tags`
- `list_bookmarks`
- `list_tasks`
- `set_task_status`
- `ensure_block_id`
- `read_canvas`
- `edit_canvas`
- `find_connections`
- `find_path_between`
- `rank_notes`
- `reindex`
- `index_status`
- `find_broken_links`
- `find_orphaned_notes`
- `search_and_replace`
- `rename_tag`
- `rename_heading`

Alternatively, open the Jan MCP panel — it lists the tools once `tools/list` succeeds.

## Why not HTTP?

Short version: Jan's HTTP MCP client has a bug that bites on the exact call pattern most MCP servers use. Stdio sidesteps it entirely.

Long version:

- Jan 0.7.x ships [`rmcp`](https://github.com/modelcontextprotocol/rust-sdk) (the Rust MCP SDK) as its client.
- `rmcp`'s `StreamableHttpClientTransport` has an open bug in its SSE frame parsing — see [rust-sdk issue #468](https://github.com/modelcontextprotocol/rust-sdk/issues/468). It reads the first SSE `data:` frame as the JSON-RPC response and then gets confused about stream lifetime when the server closes the HTTP response per-request.
- Most MCP servers that speak Streamable HTTP (including `aaronsb/obsidian-mcp-plugin`) return a response as a single SSE frame and then close the stream. That is correct per spec. It also trips the rmcp bug. The symptom: `initialize` succeeds, then `tools/list` dies with `Transport closed`, and the session never recovers.
- obsidian-brain avoids the whole problem: it is stdio-only. Newline-delimited JSON over pipes — no SSE frames, no stream lifetime to misparse. Jan's stdio client is a different code path without the bug.

**For Jan, always use stdio.** The fix will eventually land when Jan bumps its `rmcp` dependency past PR #467, but you don't need to wait.

## Troubleshooting

### Tools list is empty but Jan shows the server as connected

Usually this means the server crashed after `initialize`, or first-boot indexing is still running (30–60 s on first run while the embedding model downloads). Two ways to diagnose:

1. Check Jan's MCP server log: **Settings -> MCP Servers -> click the server -> View logs**. Look for the `obsidian-brain: indexed N notes` line on stderr — if you don't see it, the first-boot index is still running.
2. Run the server by hand and watch stderr:

   ```bash
   obsidian-brain server --vault notes=/absolute/path/to/your/vault
   ```

   Paste a single `initialize` frame on stdin and confirm you get a JSON-RPC response back.

### `command not found: obsidian-brain`

You used a bare `obsidian-brain` instead of the absolute path. Replace with the output of `which obsidian-brain` (typically `/opt/homebrew/bin/obsidian-brain` on macOS Homebrew).

### `Give at least one vault with --vault <name=path>.`

The arguments in Jan's UI didn't stick. Open the server entry, re-enter `server`, `--vault`, `notes=/absolute/path/to/your/vault` as separate arguments, and save.

### `better-sqlite3` ABI mismatch (`ERR_DLOPEN_FAILED`)

The native module was built against a different Node ABI than the one Jan launches. Rebuild against the same Node:

```bash
PATH=/opt/homebrew/bin:$PATH npm rebuild better-sqlite3
```

### Slow first call / Jan times out on first startup

The server auto-indexes on first boot and downloads the default embedding model (~34 MB). If Jan's spawn timeout is shorter than this (some versions: 30 s) the first connection attempt may fail. Warm the index from a shell first so the model is cached locally:

```bash
obsidian-brain index --vault notes=/absolute/path/to/your/vault
```

After that, subsequent connections from Jan start in well under a second.
