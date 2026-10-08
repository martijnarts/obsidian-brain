/**
 * Snapshot tests for `obsidian-brain --help` output.
 *
 * **What this catches**: drift between CLI help text and what subcommands
 * actually accept. If a developer adds/removes a flag, changes a default,
 * rewords a description, or adds a new subcommand, the snapshot diff is
 * visible in their PR. Forces an explicit "yes I meant to change this".
 *
 * **What this does NOT catch**: lies that have always been lies. The
 * snapshot just freezes whatever's currently there — it's a forcing
 * function for change-noise, not a correctness oracle. (See `docs/cli.md`
 * for the prose-level user-facing reference and verify it stays in sync.)
 *
 * **How to update after intentional CLI changes**: re-run `vitest -u`
 * (or `npm test -- -u`) to regenerate snapshots; review the diff in your
 * PR. Don't blindly accept — read every line and ask "does the help text
 * still describe what the code does?"
 */

import { describe, it, expect } from 'vitest';
import { buildProgram } from '../../src/cli/index.js';
import type { Command } from 'commander';

/** Pull a subcommand by name from the top-level program. */
function getSubcommand(program: Command, name: string): Command {
  const cmd = program.commands.find((c) => c.name() === name);
  if (!cmd) throw new Error(`subcommand '${name}' not found`);
  return cmd;
}

describe('CLI help-text snapshots', () => {
  it('top-level `--help` lists every subcommand we expect', () => {
    const program = buildProgram();
    const help = program.helpInformation();
    // Strip the version line because it tracks package.json (changes
    // every release; not what we're snapshotting here).
    expect(help).toMatchInlineSnapshot(`
      "Usage: obsidian-brain [options] [command]

      Semantic search + knowledge graph + vault editing for Obsidian.

      Options:
        -v, --version             output the version number
        -h, --help                display help for command

      Commands:
        server [options]          Start the MCP server. Every tool takes a required
                                  \`vault\` argument naming one of the --vault names.
                                  Indexes live in <DATA_DIR>/<name>.
        index [options]           Scan each vault and update its knowledge-graph index
                                  (incremental), one vault at a time
        watch [options]           Long-running process: keep each vault index live by
                                  reindexing on vault changes. Use this if you want to
                                  run the watcher independently from an MCP client
                                  (via launchd/systemd).
        search [options] <query>  Hybrid (default), semantic, or full-text search over
                                  one vault
        models                    Inspect and manage embedding models
        help [command]            display help for command
      "
    `);
  });

  it('`server --help` lists the vault, transport and listen options', () => {
    const cmd = getSubcommand(buildProgram(), 'server');
    expect(cmd.helpInformation()).toMatchInlineSnapshot(`
      "Usage: obsidian-brain server [options]

      Start the MCP server. Every tool takes a required \`vault\` argument naming one of
      the --vault names. Indexes live in <DATA_DIR>/<name>.

      Options:
        --vault <name=path>      A vault to serve. Repeat for more vaults. At least
                                 one is required.
        --transport <transport>  stdio for an MCP client that spawns the server; http
                                 for a long-running server at /mcp (choices: "stdio",
                                 "http", default: "stdio")
        --listen <host:port>     Address to listen on with --transport http (default:
                                 127.0.0.1:8080)
        -h, --help               display help for command
      "
    `);
  });

  it('`index --help` documents --drop accurately (regression: had claimed "required for model switches")', () => {
    const cmd = getSubcommand(buildProgram(), 'index');
    expect(cmd.helpInformation()).toMatchInlineSnapshot(`
      "Usage: obsidian-brain index [options]

      Scan each vault and update its knowledge-graph index (incremental), one vault at
      a time

      Options:
        --vault <name=path>   A vault to index. Repeat for more vaults. At least one
                              is required.
        -r, --resolution <n>  Louvain resolution (passing this forces a
                              community-cache refresh even if no files changed)
        --drop                Drop all embeddings + sync state before indexing. Mostly
                              an escape hatch — the bootstrap auto-detects
                              EMBEDDING_MODEL / EMBEDDING_PROVIDER changes and wipes
                              embedding state on its own; \`--drop\` is for forcing a
                              from-scratch rebuild when something else has gone wrong.
                              (default: false)
        -h, --help            display help for command
      "
    `);
  });

  it('`watch --help`', () => {
    const cmd = getSubcommand(buildProgram(), 'watch');
    expect(cmd.helpInformation()).toMatchInlineSnapshot(`
      "Usage: obsidian-brain watch [options]

      Long-running process: keep each vault index live by reindexing on vault changes.
      Use this if you want to run the watcher independently from an MCP client (via
      launchd/systemd).

      Options:
        --vault <name=path>        A vault to watch. Repeat for more vaults. At least
                                   one is required.
        --debounce <ms>            Per-file reindex debounce (ms) (default: 3000)
        --community-debounce <ms>  Graph-wide community detection debounce (ms)
                                   (default: 60000)
        -h, --help                 display help for command
      "
    `);
  });

  it('`search --help` lists hybrid as default (regression: had been missing the production-default mode entirely)', () => {
    const cmd = getSubcommand(buildProgram(), 'search');
    expect(cmd.helpInformation()).toMatchInlineSnapshot(`
      "Usage: obsidian-brain search [options] <query>

      Hybrid (default), semantic, or full-text search over one vault

      Options:
        --vault <name=path>  The vault to search. Exactly one is required.
        -l, --limit <n>      Max results (default: 10)
        -m, --mode <mode>    hybrid (RRF-fused, the production default) | semantic |
                             fulltext (default: "hybrid")
        -h, --help           display help for command
      "
    `);
  });

  it('`models --help` lists every subcommand including the user-config layer (add/override/fetch-seed/refresh-cache)', () => {
    const cmd = getSubcommand(buildProgram(), 'models');
    expect(cmd.helpInformation()).toMatchInlineSnapshot(`
      "Usage: obsidian-brain models [options] [command]

      Inspect and manage embedding models

      Options:
        -h, --help               display help for command

      Commands:
        list [options]           List embedding models. By default shows the 6
                                 hardcoded presets; pass --all to surface every entry
                                 in the bundled MTEB-derived seed (~348 models).
                                 --filter narrows by substring on model id.
        recommend <vault-path>   Inspect the vault folder and recommend the best
                                 embedding preset.
        prefetch [preset]        Warm the HF cache for a preset's model. Defaults to
                                 the "english" preset.
        check [options] <id>     Fetch model metadata from HF without downloading the
                                 model (~1s). Add --load to also download + load via
                                 transformers.js (~30s).
        refresh-cache [options]  Invalidate the metadata cache of every vault index in
                                 DATA_DIR so the next server boot refetches from the
                                 seed → HF chain. Cheap for seeded models (~0 HF calls
                                 — the bundled seed repopulates the cache instantly);
                                 1 HF call per non-seeded BYOM id. The prefix-strategy
                                 hash auto-detects any prefix change and triggers a
                                 re-embed in bootstrap, so it is safe to run any time
                                 you suspect cached metadata is stale. Restart the
                                 server after running this. Caveat: if you run it
                                 OFFLINE on a non-seeded BYOM id, fallback safe
                                 defaults get cached — fix by running again online or
                                 editing the override file (\`models override\`).
        add [options] <id>       Register a new model not in the bundled seed.
                                 Required: --max-tokens. Optional: --query-prefix,
                                 --document-prefix (default ""). Asserts the id is not
                                 already in the seed (use \`models override\` for
                                 existing ids). Writes to
                                 ~/.config/obsidian-brain/model-overrides.json;
                                 survives \`npm update\`. Restart the server after
                                 running this.
        override [options] [id]  Set, remove, or list user-controlled metadata
                                 overrides at
                                 ~/.config/obsidian-brain/model-overrides.json.
                                 Survives \`npm update\`. Use to correct upstream
                                 MTEB/HF errors locally — e.g. \`models override
                                 BAAI/bge-small-en-v1.5 --max-tokens 1024\`. Restart
                                 the server after running this; prefix changes
                                 auto-trigger a re-embed via the prefix-strategy hash
                                 in bootstrap.
        fetch-seed [options]     Download the latest data/seed-models.json from the
                                 obsidian-brain main branch on GitHub. Bypasses
                                 waiting for an npm release when MTEB ships an
                                 upstream fix. Writes to
                                 ~/.config/obsidian-brain/seed-models.json; the
                                 seed-loader picks it up automatically over the
                                 bundled package copy. Pass --check to validate the
                                 download without writing.
        help [command]           display help for command
      "
    `);
  });

  it('`models prefetch --help` does NOT list --timeout (regression: option was declared but `void`-ed)', () => {
    const program = buildProgram();
    const models = getSubcommand(program, 'models');
    const prefetch = getSubcommand(models, 'prefetch');
    const help = prefetch.helpInformation();
    expect(help).not.toContain('--timeout');
    expect(help).toMatchInlineSnapshot(`
      "Usage: obsidian-brain models prefetch [options] [preset]

      Warm the HF cache for a preset's model. Defaults to the \"english\" preset.

      Options:
        -h, --help  display help for command
      "
    `);
  });

  it('`models check --help` documents both --timeout and --load (real options that work)', () => {
    const program = buildProgram();
    const models = getSubcommand(program, 'models');
    const check = getSubcommand(models, 'check');
    expect(check.helpInformation()).toMatchInlineSnapshot(`
      "Usage: obsidian-brain models check [options] <id>

      Fetch model metadata from HF without downloading the model (~1s). Add --load to
      also download + load via transformers.js (~30s).

      Options:
        --timeout <ms>  HTTP timeout in ms (default: 10000)
        --load          Also download + load the model (slow) (default: false)
        -h, --help      display help for command
      "
    `);
  });

  it('`models refresh-cache --help` documents --model and the restart hint', () => {
    const program = buildProgram();
    const models = getSubcommand(program, 'models');
    const refresh = getSubcommand(models, 'refresh-cache');
    expect(refresh.helpInformation()).toMatchInlineSnapshot(`
      "Usage: obsidian-brain models refresh-cache [options]

      Invalidate the metadata cache of every vault index in DATA_DIR so the next
      server boot refetches from the seed → HF chain. Cheap for seeded models (~0 HF
      calls — the bundled seed repopulates the cache instantly); 1 HF call per
      non-seeded BYOM id. The prefix-strategy hash auto-detects any prefix change and
      triggers a re-embed in bootstrap, so it is safe to run any time you suspect
      cached metadata is stale. Restart the server after running this. Caveat: if you
      run it OFFLINE on a non-seeded BYOM id, fallback safe defaults get cached — fix
      by running again online or editing the override file (\`models override\`).

      Options:
        --model <id>  Refresh cache for one model id only (default: all entries)
        -h, --help    display help for command
      "
    `);
  });

  it('every advertised subcommand under `models` actually resolves', () => {
    // Catches the bug class where the parent help advertises a
    // subcommand that doesn't exist (or vice-versa).
    const program = buildProgram();
    const models = getSubcommand(program, 'models');
    const expected = ['list', 'recommend', 'prefetch', 'check', 'refresh-cache'];
    const actual = models.commands.map((c) => c.name());
    for (const name of expected) {
      expect(actual, `models.${name} missing`).toContain(name);
    }
  });
});
