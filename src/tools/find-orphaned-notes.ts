import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';
import { listNotePaths } from '../vault/scan.js';

/**
 * `find_orphaned_notes` — notes that no other note links to. Works from the
 * edge table alone: a self-link does not count, and a link from an excluded
 * folder still counts (exclusion only hides notes from the result).
 */
export function registerFindOrphanedNotesTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'find_orphaned_notes',
    'List notes with no incoming wikilinks from other notes (self-links do not count). Read-only. Returns path, title and outgoing link count per orphan.',
    {
      folder: z.string().optional().describe('Only report notes under this folder.'),
      excludeFolders: z.array(z.string()).optional().describe('Folders whose notes are not reported. Their links still count.'),
      limit: z.number().int().positive().optional().describe('Max orphans returned. Default 100.'),
    },
    async (args) => {
      const linked = new Set(
        (
          ctx.db
            .prepare('SELECT DISTINCT target_id FROM edges WHERE source_id != target_id')
            .all() as Array<{ target_id: string }>
        ).map((r) => r.target_id),
      );
      const titleOf = ctx.db.prepare('SELECT title FROM nodes WHERE id = ?');
      const outgoingOf = ctx.db.prepare(
        'SELECT COUNT(*) AS n FROM edges WHERE source_id = ? AND target_id != source_id',
      );

      const orphans = listNotePaths(ctx.db, args.folder, args.excludeFolders).filter((id) => !linked.has(id));
      const cap = args.limit ?? 100;
      return {
        total: orphans.length,
        ...(orphans.length > cap ? { truncated: true } : {}),
        orphans: orphans.slice(0, cap).map((path) => ({
          path,
          title: (titleOf.get(path) as { title: string }).title,
          outgoingLinks: (outgoingOf.get(path) as { n: number }).n,
        })),
      };
    },
  );
}
