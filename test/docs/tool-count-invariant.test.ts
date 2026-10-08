import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import type { ServerContext } from '../../src/context.js';
import { registerTools } from '../../src/server.js';
import { VaultTools } from '../../src/vaults.js';

/**
 * Doc-drift invariant: docs/tools.md must list exactly the tools the server
 * registers. Catches the kind of drift where a new tool was added in source
 * but not surfaced in the docs (or removed from source but still
 * documented).
 *
 * The tool list comes from `VaultTools`, the same set the server exposes:
 * every tool from `registerTools`, plus `list_vaults`. Handlers never run,
 * so an empty context is enough.
 */
describe('docs/tools.md vs registered tools — drift invariant', () => {
  const registered = new VaultTools(
    [{ name: 'notes', ctx: {} as ServerContext, watcher: null }],
    registerTools,
  ).names();

  it('docs/tools.md headings match every registered MCP tool, no extras, no omissions', () => {
    const md = readFileSync('docs/tools.md', 'utf8');
    const docTools = [...md.matchAll(/^### `([a-z_]+)`/gm)].map((m) => m[1]).sort();

    expect(docTools, 'docs/tools.md tool list drifted from the registered tools').toEqual(
      [...registered].sort(),
    );
  });

  it('docs/tools.md frontmatter description count matches the actual tool count', () => {
    const md = readFileSync('docs/tools.md', 'utf8');
    const m = md.match(/All (\d+) MCP tools/);
    expect(m, 'docs/tools.md frontmatter must claim a tool count').not.toBeNull();
    expect(parseInt(m![1], 10), 'docs/tools.md frontmatter tool count drifted').toBe(
      registered.length,
    );
  });
});
