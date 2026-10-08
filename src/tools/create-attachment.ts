import { z } from 'zod';
import { mkdir, rename, stat, writeFile } from 'node:fs/promises';
import { dirname } from 'node:path';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';
import { resolveVaultPath } from '../vault/vault-path.js';

const MAX_BYTES = 10 * 1024 * 1024;

/**
 * `create_attachment` — write a binary file from base64. The index holds
 * only notes, so no reindex follows: a link to the new file was already a
 * plain edge or stub, and stays one.
 */
export function registerCreateAttachmentTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'create_attachment',
    'Create a binary file (image, PDF, ...) from base64 content, with any missing parent folders. Max 10 MB decoded. Use `create_note` for markdown.',
    {
      path: z.string().describe('Vault-relative file path with extension, e.g. `assets/diagram.png`.'),
      content: z.string().describe('File bytes, base64-encoded.'),
      overwrite: z.boolean().optional().describe('Replace an existing file. Default false.'),
    },
    async (args) => {
      const { rel, abs } = resolveVaultPath(ctx.config.vaultPath, args.path);
      if (rel === '') throw new Error('Path is the vault root: give a file path');
      if (rel.toLowerCase().endsWith('.md')) {
        throw new Error('Refusing to write a .md file: use create_note for notes');
      }

      const b64 = args.content.replace(/\s+/g, '');
      if (!/^[A-Za-z0-9+/]*={0,2}$/.test(b64) || b64.length % 4 === 1) {
        throw new Error('content is not valid base64');
      }
      const padding = b64.endsWith('==') ? 2 : b64.endsWith('=') ? 1 : 0;
      const decodedSize = Math.floor((b64.length * 3) / 4) - padding;
      if (decodedSize > MAX_BYTES) {
        throw new Error(`content is ${decodedSize} bytes decoded; the limit is ${MAX_BYTES} bytes (10 MB)`);
      }

      const existing = await stat(abs).catch(() => undefined);
      if (existing?.isDirectory()) throw new Error(`${rel} is a folder`);
      if (existing && args.overwrite !== true) {
        throw new Error(`File already exists: ${rel}. Pass overwrite: true to replace it.`);
      }

      const bytes = Buffer.from(b64, 'base64');
      await mkdir(dirname(abs), { recursive: true });
      const tmp = `${abs}.tmp`;
      await writeFile(tmp, bytes);
      await rename(tmp, abs);

      return { path: rel, bytesWritten: bytes.length, overwritten: existing !== undefined };
    },
  );
}
