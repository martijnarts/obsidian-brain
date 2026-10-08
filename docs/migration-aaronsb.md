---
title: Migrating from the aaronsb Obsidian MCP plugin
description: If you were using aaronsb/obsidian-mcp-plugin as your Claude connector, here's how to swap to obsidian-brain in 3 steps.
---

# Migrating from the aaronsb Obsidian MCP plugin

If you were using [`aaronsb/obsidian-mcp-plugin`](https://github.com/aaronsb/obsidian-mcp-plugin) as your Claude connector: obsidian-brain replaces it entirely. Three quick differences:

- Works without Obsidian running (aaronsb requires Obsidian + a plugin inside it).
- No Local REST API plugin required.
- Chunk-level semantic search with RRF hybrid retrieval (aaronsb only has keyword FTS).

## Three-step swap

1. Remove aaronsb's block from your MCP client config and add obsidian-brain's. See [Install in your MCP client](install-clients.md).
2. Disable the plugin in Obsidian (Settings → Community plugins → toggle off). Uninstall BRAT too if you don't beta-test other plugins.
3. Quit Claude Desktop (⌘Q) and relaunch.

## Feature equivalents

How aaronsb's Obsidian-process features map to obsidian-brain:

| aaronsb feature | obsidian-brain equivalent |
|---|---|
| Active editor / cursor awareness | No equivalent |
| Dataview DQL queries | `query_notes` |
| Obsidian Bases queries | No equivalent |

Inline Dataview `key:: value` fields are parsed into searchable frontmatter.
