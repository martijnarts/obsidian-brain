import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';
import { allNotes } from '../store/nodes.js';
import { countTags, sortTagCounts } from '../vault/tags.js';

export function registerListTagsTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'list_tags',
    'List every tag in the vault with the number of notes that carry it. Counts frontmatter `tags`/`tag` and inline `#tags`; tags are returned without the `#`.',
    {
      sort: z.enum(['count', 'name']).optional().describe('Default `count` (descending). `name` sorts alphabetically.'),
      limit: z.number().int().positive().optional().describe('Max tags to return. Default 100.'),
      prefix: z.string().optional().describe('Only tags starting with this text, e.g. `project/`.'),
      includeParents: z.boolean().optional().describe('Default `true`: a note tagged `a/b` also counts toward `a`.'),
    },
    async (args) => {
      const cap = args.limit ?? 100;
      const prefix = args.prefix?.replace(/^#/, '');
      const counts = countTags(
        allNotes(ctx.db).map((n) => n.frontmatter),
        args.includeParents !== false,
      );

      let tags = sortTagCounts(counts);
      if (prefix) tags = tags.filter((t) => t.tag.startsWith(prefix));
      if (args.sort === 'name') tags.sort((a, b) => a.tag.localeCompare(b.tag, 'en'));

      return {
        totalTags: tags.length,
        truncated: tags.length > cap,
        tags: tags.slice(0, cap),
      };
    },
  );
}
