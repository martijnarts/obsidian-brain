import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';
import { resolveSingleNote } from '../resolve/single-note.js';
import { KnowledgeGraph } from '../graph/builder.js';
import { findPaths, commonNeighbors } from '../graph/pathfinding.js';

export function registerFindPathBetweenTool(
  server: McpServer,
  ctx: ServerContext,
): void {
  registerTool(
    server,
    'find_path_between',
    'Find link paths between two notes. Returns all simple paths up to maxDepth edges, optionally including their shared neighbors. Broken-wikilink stub nodes are excluded by default — they are degree-1 dead ends in the undirected graph and will block legitimate paths if left in. Pass `includeStubs: true` to include them.',
    {
      from: z.string().describe('Source note (path or fuzzy match).'),
      to: z.string().describe('Target note (path or fuzzy match).'),
      maxDepth: z.number().int().positive().optional().describe('Maximum path length in hops. Default 3.'),
      includeCommon: z.boolean().optional().describe('Also return notes that both `from` and `to` link to (shared neighbors).'),
      includeStubs: z.boolean().optional().describe('Default `false`. Set `true` to include broken-wikilink stub nodes (`frontmatter._stub: true`) in the path search.'),
    },
    async (args) => {
      const { from, to, maxDepth, includeCommon, includeStubs } = args;
      const fromId = resolveSingleNote(from, ctx.db, { allowStubs: true });
      const toId = resolveSingleNote(to, ctx.db, { allowStubs: true });

      const kg = KnowledgeGraph.fromStore(ctx.db, { includeStubs });
      const g = kg.graph();
      const paths = findPaths(g, fromId, toId, maxDepth ?? 3);
      if (includeCommon) {
        const common = commonNeighbors(g, fromId, toId);
        return { paths, common };
      }
      return { paths };
    },
  );
}
