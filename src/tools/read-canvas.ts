import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';
import { canvasPath, readCanvas } from '../vault/canvas.js';

/**
 * `read_canvas` — return the nodes and edges of a `.canvas` file as stored,
 * including fields this server does not model.
 */
export function registerReadCanvasTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'read_canvas',
    'Read an Obsidian canvas (JSON Canvas) and return its nodes and edges with their ids, positions and content.',
    {
      path: z.string().min(1).describe('Vault-relative path of the `.canvas` file. The extension is optional.'),
    },
    async (args) => {
      const path = canvasPath(args.path);
      const doc = await readCanvas(ctx.config.vaultPath, path);
      if (!doc) throw new Error(`Canvas not found: ${path}`);
      return { path, nodes: doc.nodes, edges: doc.edges };
    },
  );
}
