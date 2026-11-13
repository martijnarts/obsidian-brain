# Install in your MCP client

Every client starts the same process: the `obsidian-brain` binary with `server --vault <name>=<path>`. The configs below differ only in the shape each client expects.

## The binary path

Install the binary with `nix profile add github:martijnarts/obsidian-brain`, then find its path:

```bash
command -v obsidian-brain
```

That is usually `~/.nix-profile/bin/obsidian-brain`. Use the absolute path in client configs: desktop apps do not always see the `PATH` of your shell. The examples use `/home/you/.nix-profile/bin/obsidian-brain`; replace it with your path, and replace `/absolute/path/to/your/vault` with the path of your vault.

A client can also start the server through Nix, with `nix` as the command and `run github:martijnarts/obsidian-brain -- server --vault notes=/path` as the arguments. The first start then waits for the build or download.

On the first start the server downloads the embedding model and indexes the vault in the background. Tools answer at once; semantic search reports that it is preparing until the model is ready.

Each config takes an `env` block for the variables in [Configuration](configuration.md), such as `"EMBEDDING_PRESET": "multilingual"`.

## Claude Desktop

Edit `~/Library/Application Support/Claude/claude_desktop_config.json` on macOS, or open Settings, Developer, Edit Config:

```json
{
  "mcpServers": {
    "obsidian-brain": {
      "command": "/home/you/.nix-profile/bin/obsidian-brain",
      "args": ["server", "--vault", "notes=/absolute/path/to/your/vault"]
    }
  }
}
```

Quit Claude Desktop and start it again.

## Claude Code

```bash
claude mcp add --scope user --transport stdio obsidian-brain \
  -- /home/you/.nix-profile/bin/obsidian-brain server --vault notes="$HOME/path/to/your/vault"
```

## Cursor, Windsurf, Cline, LM Studio, JetBrains AI Assistant, Gemini CLI

These clients use the `mcpServers` shape of Claude Desktop. The config files are:

| Client | File |
|---|---|
| Cursor | `~/.cursor/mcp.json`, or `.cursor/mcp.json` in a project |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` |
| Cline | `cline_mcp_settings.json`, from the MCP Servers panel |
| LM Studio | `~/.lmstudio/mcp.json` |
| JetBrains AI Assistant | Settings, Tools, AI Assistant, Model Context Protocol, Add, As JSON |
| Gemini CLI | `~/.gemini/settings.json` |

## VS Code

`.vscode/mcp.json` in a workspace uses `servers` and `type`:

```json
{
  "servers": {
    "obsidian-brain": {
      "type": "stdio",
      "command": "/home/you/.nix-profile/bin/obsidian-brain",
      "args": ["server", "--vault", "notes=/absolute/path/to/your/vault"]
    }
  }
}
```

## Zed

`~/.config/zed/settings.json` uses `context_servers`:

```json
{
  "context_servers": {
    "obsidian-brain": {
      "source": "custom",
      "command": "/home/you/.nix-profile/bin/obsidian-brain",
      "args": ["server", "--vault", "notes=/absolute/path/to/your/vault"]
    }
  }
}
```

## Opencode

`opencode.json` uses `mcp`, with the command as an array:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "obsidian-brain": {
      "type": "local",
      "command": ["/home/you/.nix-profile/bin/obsidian-brain", "server", "--vault", "notes=/absolute/path/to/your/vault"],
      "enabled": true
    }
  }
}
```

## Codex CLI

```bash
codex mcp add obsidian-brain -- /home/you/.nix-profile/bin/obsidian-brain server --vault notes="$HOME/path/to/your/vault"
```

## Jan

In Settings, MCP Servers, add a server with the `STDIO` transport. The command is the binary path; the arguments are `server`, `--vault` and `notes=/absolute/path/to/your/vault`.

## A remote server over HTTP

Run the server with `--transport http` behind an authenticating proxy, as the README describes. Then give the client the proxy URL ending in `/mcp`. The exact config depends on the client and the proxy.

## Check the connection

After a restart, the client lists the server with 41 tools. Ask it to call `list_vaults`: the answer names each vault and its index state. If the server does not appear, see [Troubleshooting](troubleshooting.md).
