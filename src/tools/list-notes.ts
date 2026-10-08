import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';
import { allNodeIds, getNode } from '../store/nodes.js';
import { allSyncMtimes } from '../store/sync.js';
import { noteTags, tagMatches } from '../vault/tags.js';

export function registerListNotesTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'list_notes',
    'List notes in the vault. Optionally filter by directory prefix or by tag (frontmatter or inline; `a` also matches `a/b`). Pass `sortBy: "mtime"` for the most recently modified notes first. Pass `includeStubs: false` to exclude unresolved wiki-link targets (nodes with `frontmatter._stub: true`) and see only real on-disk notes.',
    {
      directory: z.string().optional().describe('Restrict to notes under this subdirectory prefix.'),
      tag: z.string().optional().describe('Restrict to notes with this tag or a tag nested below it.'),
      sortBy: z.enum(['path', 'mtime']).optional().describe('Default `path`. `mtime` sorts newest first and adds `mtime` to each result.'),
      limit: z.number().int().positive().optional().describe('Max results to return. Default 100.'),
      includeStubs: z.boolean().optional().describe('Default `true`. Set `false` to exclude unresolved wiki-link targets.'),
    },
    async (args) => {
      const { directory, tag, sortBy, limit, includeStubs } = args;
      const ids = allNodeIds(ctx.db).sort();
      const results: Array<{
        id: string;
        title: string;
        tags: string[];
        frontmatter: Record<string, unknown>;
        mtime?: string;
      }> = [];
      const cap = limit ?? 100;
      const excludeStubs = includeStubs === false;
      const byMtime = sortBy === 'mtime';

      for (const id of ids) {
        if (directory !== undefined) {
          if (!(id.startsWith(directory + '/') || id === directory)) continue;
        }
        const node = getNode(ctx.db, id);
        if (!node) continue;
        if (excludeStubs && node.frontmatter._stub === true) continue;
        const tags = Array.isArray(node.frontmatter.tags)
          ? (node.frontmatter.tags as string[])
          : [];
        if (tag !== undefined && !noteTags(node.frontmatter).some((t) => tagMatches(t, tag))) continue;

        results.push({
          id: node.id,
          title: node.title,
          tags,
          frontmatter: node.frontmatter,
        });
        if (!byMtime && results.length >= cap) break;
      }

      if (!byMtime) return results;

      // Notes the indexer has not recorded yet (stubs, fresh writes) sort last.
      const mtimes = allSyncMtimes(ctx.db);
      return results
        .map((r) => ({ r, ms: mtimes.get(r.id) ?? 0 }))
        .sort((a, b) => b.ms - a.ms || a.r.id.localeCompare(b.r.id))
        .slice(0, cap)
        .map(({ r, ms }) => ({ ...r, mtime: ms > 0 ? new Date(ms).toISOString() : undefined }));
    },
  );
}
