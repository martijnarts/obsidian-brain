import { z } from 'zod';
import { promises as fs } from 'node:fs';
import { join, relative, isAbsolute } from 'node:path';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import { runBackgroundReindex } from './background-reindex.js';
import type { ServerContext } from '../context.js';
import { resolveToSinglePath } from './delete-note.js';
import { getNode } from '../store/nodes.js';
import { updateFrontmatter } from '../vault/editor.js';

/** Throws unless `abs`, after following symlinks, lies inside the vault. */
async function assertInsideVault(vaultPath: string, abs: string, relPath: string): Promise<void> {
  const [root, target] = await Promise.all([fs.realpath(vaultPath), fs.realpath(abs)]);
  const rel = relative(root, target);
  if (rel === '' || rel.startsWith('..') || isAbsolute(rel)) {
    throw new Error(`Path escapes the vault: ${relPath}`);
  }
}

export function registerUpdatePropertiesTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'update_properties',
    'Set and remove several frontmatter properties of one note in a single write. Creates the frontmatter block if the note has none; the body and keys not named stay as they are. Pass `dryRun: true` to see the resulting frontmatter without writing.',
    {
      name: z.string().describe('Path or fuzzy match of the note.'),
      set: z.record(z.string(), z.unknown()).optional().describe('Keys to write, with their values. A `null` value removes the key.'),
      remove: z.array(z.string()).optional().describe('Keys to remove. Absent keys are skipped.'),
      dryRun: z.boolean().optional().describe('Return the new frontmatter without writing.'),
    },
    async (args) => {
      const set = args.set ?? {};
      const remove = args.remove ?? [];
      if (Object.keys(set).length === 0 && remove.length === 0) {
        throw new Error('Nothing to do: pass at least one key in `set` or `remove`.');
      }
      const invalid = [...Object.keys(set), ...remove].filter((k) => k.trim() === '' || /[\r\n]/.test(k));
      if (invalid.length > 0) {
        throw new Error(`Invalid frontmatter key(s): ${invalid.map((k) => JSON.stringify(k)).join(', ')}`);
      }

      const fileRelPath = resolveToSinglePath(args.name, ctx);
      if (getNode(ctx.db, fileRelPath)?.frontmatter._stub === true) {
        throw new Error(`"${fileRelPath}" is an unresolved link target, not a note on disk.`);
      }
      const abs = join(ctx.config.vaultPath, fileRelPath);
      await assertInsideVault(ctx.config.vaultPath, abs, fileRelPath);

      const original = await fs.readFile(abs, 'utf-8');
      const res = updateFrontmatter(original, set, remove);
      const summary = {
        path: fileRelPath,
        set: res.set,
        removed: res.removed,
        ...(res.notPresent.length > 0 ? { notPresent: res.notPresent } : {}),
        frontmatter: res.frontmatter,
      };
      if (args.dryRun === true) return { dryRun: true, ...summary };

      if (res.next !== original) {
        const tmp = `${abs}.tmp`;
        await fs.writeFile(tmp, res.next, 'utf-8');
        await fs.rename(tmp, abs);
        runBackgroundReindex(ctx);
      }
      return summary;
    },
  );
}
