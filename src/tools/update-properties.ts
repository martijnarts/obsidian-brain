import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import { runBackgroundReindex } from './background-reindex.js';
import type { ServerContext } from '../context.js';
import { resolveSingleNote } from '../resolve/single-note.js';
import { updateFrontmatter } from '../vault/editor.js';
import { readNoteFile, writeFileAtomic } from '../vault/vault-path.js';

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

      const fileRelPath = resolveSingleNote(args.name, ctx.db);
      const { abs, content: original } = await readNoteFile(ctx.config.vaultPath, fileRelPath);
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
        await writeFileAtomic(abs, res.next);
        runBackgroundReindex(ctx);
      }
      return summary;
    },
  );
}
