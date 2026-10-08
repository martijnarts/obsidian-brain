import { z } from 'zod';
import { readdir } from 'node:fs/promises';
import { join } from 'node:path';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';
import { allNotes } from '../store/nodes.js';
import { allSyncMtimes } from '../store/sync.js';
import { countTags, sortTagCounts } from '../vault/tags.js';

/** Non-Markdown files on disk, skipping hidden entries like `.obsidian/`. */
async function countAttachments(dir: string): Promise<number> {
  let n = 0;
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    if (entry.name.startsWith('.')) continue;
    if (entry.isDirectory()) n += await countAttachments(join(dir, entry.name));
    else if (entry.isFile() && !entry.name.endsWith('.md')) n++;
  }
  return n;
}

export function registerVaultOverviewTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'vault_overview',
    'One-call orientation for an unfamiliar vault: note and attachment counts, notes per top-level folder, the most used tags, and the most recently modified notes. Use `list_tags` or `list_notes` for more than the snapshot.',
    {
      topTags: z.number().int().positive().optional().describe('Tags to include. Default 15.'),
      recent: z.number().int().positive().optional().describe('Recently modified notes to include. Default 10.'),
    },
    async (args) => {
      const notes = allNotes(ctx.db);

      const folderCounts = new Map<string, number>();
      for (const n of notes) {
        const slash = n.id.indexOf('/');
        const folder = slash === -1 ? '(root)' : n.id.slice(0, slash);
        folderCounts.set(folder, (folderCounts.get(folder) ?? 0) + 1);
      }
      const folders = [...folderCounts]
        .map(([folder, count]) => ({ folder, count }))
        .sort((a, b) => b.count - a.count || a.folder.localeCompare(b.folder, 'en'));

      const topTags = sortTagCounts(countTags(notes.map((n) => n.frontmatter), true))
        .slice(0, args.topTags ?? 15);

      const mtimes = allSyncMtimes(ctx.db);
      const recent = notes
        .filter((n) => (mtimes.get(n.id) ?? 0) > 0)
        .sort((a, b) => mtimes.get(b.id)! - mtimes.get(a.id)! || a.id.localeCompare(b.id))
        .slice(0, args.recent ?? 10)
        .map((n) => ({ id: n.id, title: n.title, mtime: new Date(mtimes.get(n.id)!).toISOString() }));

      return {
        notes: notes.length,
        attachments: await countAttachments(ctx.config.vaultPath),
        folders,
        topTags,
        recent,
      };
    },
  );
}
