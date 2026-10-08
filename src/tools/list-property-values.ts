import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';
import { allNotes } from '../store/nodes.js';

export function registerListPropertyValuesTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'list_property_values',
    'List the distinct values of one frontmatter property across notes, with how many times each occurs. Each element of a list value counts separately. Also reports how many notes have the key.',
    {
      key: z.string().min(1).describe('Frontmatter key, e.g. `status`.'),
      folder: z.string().optional().describe('Only notes under this folder.'),
      limit: z.number().int().positive().optional().describe('Max distinct values to return. Default 100.'),
    },
    async (args) => {
      const { key } = args;
      const cap = args.limit ?? 100;
      const folder = args.folder?.replace(/\/+$/, '');

      // Keyed by JSON so the number 5 and the string "5" stay distinct.
      const counts = new Map<string, { value: unknown; count: number }>();
      let notesWithKey = 0;
      for (const n of allNotes(ctx.db)) {
        if (folder && !n.id.startsWith(folder + '/')) continue;
        if (!Object.prototype.hasOwnProperty.call(n.frontmatter, key)) continue;
        notesWithKey++;
        const raw = n.frontmatter[key];
        for (const value of Array.isArray(raw) ? raw : [raw]) {
          const k = JSON.stringify(value) ?? 'null';
          const entry = counts.get(k);
          if (entry) entry.count++;
          else counts.set(k, { value, count: 1 });
        }
      }

      const values = [...counts]
        .sort(([ka, a], [kb, b]) => b.count - a.count || ka.localeCompare(kb, 'en'))
        .map(([, v]) => v);

      return {
        key,
        notesWithKey,
        totalDistinct: values.length,
        truncated: values.length > cap,
        values: values.slice(0, cap),
      };
    },
  );
}
