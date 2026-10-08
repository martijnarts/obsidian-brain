import { z } from 'zod';
import { mkdir, stat } from 'node:fs/promises';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';
import { resolveVaultPath } from '../vault/vault-path.js';

/**
 * `create_folder` — make a folder and any missing parents. An existing
 * folder is not an error (`created: false`); an existing file at the path
 * is. An empty folder holds no notes, so the index has nothing to update.
 */
export function registerCreateFolderTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'create_folder',
    'Create a folder in the vault, with any missing parent folders. Succeeds with `created: false` when the folder already exists.',
    {
      path: z.string().describe('Vault-relative folder path, e.g. `Projects/2026`.'),
    },
    async (args) => {
      const { rel, abs } = resolveVaultPath(ctx.config.vaultPath, args.path);
      if (rel === '') throw new Error('Path is the vault root, which already exists');

      const existing = await stat(abs).catch(() => undefined);
      if (existing?.isDirectory()) return { path: rel, created: false };
      if (existing) throw new Error(`A file already exists at ${rel}`);

      await mkdir(abs, { recursive: true });
      return { path: rel, created: true };
    },
  );
}
