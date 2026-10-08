import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import { runBackgroundReindex } from './background-reindex.js';
import type { ServerContext } from '../context.js';
import { resolveSingleNote } from '../resolve/single-note.js';

/**
 * `link_notes` — append a wiki-link from a source note to a target ref with
 * a short context sentence. Target is allowed to be an unknown/stub ref;
 * only the source has to exist in the index.
 */
export function registerLinkNotesTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'link_notes',
    "Add a wiki-link from one note to another with a context sentence describing why they're connected. Appends the link to the source note and records the edge in the graph so analytics pick it up.",
    {
      source: z.string().describe('Source note to add the link from (path or fuzzy match).'),
      target: z.string().describe('Target note to link to (path, title, or new wiki-link ref).'),
      context: z.string().min(1).describe('One-sentence explanation of why these notes are connected.'),
      dryRun: z.boolean().optional().describe('If true, return the line that would be appended without writing.'),
    },
    async (args) => {
      const { source, target, context, dryRun } = args;

      const sourceId = resolveSingleNote(source, ctx.db);

      if (dryRun === true) {
        // Mirror the line that addLink would append (writer.ts line 72).
        const wouldAppend = `\n${context} [[${target}]]`;
        return { dryRun: true, source: sourceId, target, context, wouldAppend };
      }

      ctx.writer.addLink(sourceId, target, context);

      const payload = { source: sourceId, target, context };

      // Fire-and-forget reindex: the write has already succeeded; blocking on
      // the embedder init + index run would make this tool call wait minutes on
      // first run, which MCP clients time out. The watcher path already accepts
      // this eventual-consistency window; this matches.
      runBackgroundReindex(ctx);

      return payload;
    },
  );
}
