import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';
import { resolveSingleNote } from '../resolve/single-note.js';
import { getNode } from '../store/nodes.js';
import { errorMessage } from '../util/errors.js';

const MAX_NOTES = 20;

/**
 * `read_notes` — read several notes in one call. Each name resolves on its
 * own, so a missing or ambiguous name yields an `{name, error}` entry in
 * its slot and the rest of the batch still returns.
 */
export function registerReadNotesTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'read_notes',
    `Read up to ${MAX_NOTES} notes in one call. Returns one entry per name, in order: \`{path, title, frontmatter, content, truncated}\`, or \`{name, error}\` when that name does not resolve.`,
    {
      names: z.array(z.string()).min(1).max(MAX_NOTES).describe('Paths, filenames, or fuzzy matches of the notes to read.'),
      maxContentLength: z.number().int().positive().optional().describe('Max body chars per note before truncation. Default 2000.'),
    },
    async (args) => {
      const max = args.maxContentLength ?? 2000;
      const notes = args.names.map((name) => {
        try {
          const path = resolveSingleNote(name, ctx.db);
          const node = getNode(ctx.db, path);
          if (!node) throw new Error(`No note found matching "${name}"`);
          const truncated = node.content.length > max;
          return {
            path,
            title: node.title,
            frontmatter: node.frontmatter,
            content: truncated ? node.content.slice(0, max) : node.content,
            truncated,
          };
        } catch (err) {
          return { name, error: errorMessage(err) };
        }
      });
      return { notes };
    },
  );
}
