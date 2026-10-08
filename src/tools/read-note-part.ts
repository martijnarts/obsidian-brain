import { z } from 'zod';
import { readFile } from 'node:fs/promises';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';
import { resolveSingleNote } from '../resolve/single-note.js';
import { resolveVaultPath } from '../vault/vault-path.js';
import { extractHeadings, findBlock, findHeadingSection, sliceLines } from '../vault/note-parts.js';

/**
 * `read_note_part` — read one part of a note from disk, so a client can
 * look at a long note's outline and then fetch only the section it needs.
 * Line numbers are 1-based and count the frontmatter.
 */
export function registerReadNotePartTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'read_note_part',
    'Read part of a note from disk: its heading `outline`, the section under a `heading`, a range of `lines`, or the `block` that carries a `^blockId`. Line numbers are 1-based and count the frontmatter.',
    {
      name: z.string().describe('Path, filename, or fuzzy match for the note.'),
      mode: z.enum(['outline', 'heading', 'lines', 'block']).describe('Which part to read.'),
      heading: z.string().optional().describe('For `heading`: heading text, or a nested path like `Parent > Child`.'),
      startLine: z.number().int().positive().optional().describe('For `lines`: first line, 1-based, inclusive.'),
      endLine: z.number().int().positive().optional().describe('For `lines`: last line, inclusive. Default and maximum: the end of the note.'),
      blockId: z.string().optional().describe('For `block`: the block id, with or without the leading `^`.'),
    },
    async (args) => {
      const path = resolveSingleNote(args.name, ctx.db);
      const raw = await readFile(resolveVaultPath(ctx.config.vaultPath, path).abs, 'utf-8');

      switch (args.mode) {
        case 'outline':
          return { path, headings: extractHeadings(raw) };
        case 'heading':
          if (args.heading === undefined) throw new Error('mode "heading" needs `heading`');
          return { path, ...findHeadingSection(raw, args.heading) };
        case 'lines':
          if (args.startLine === undefined) throw new Error('mode "lines" needs `startLine`');
          return { path, ...sliceLines(raw, args.startLine, args.endLine) };
        case 'block':
          if (args.blockId === undefined) throw new Error('mode "block" needs `blockId`');
          return { path, ...findBlock(raw, args.blockId) };
      }
    },
  );
}
