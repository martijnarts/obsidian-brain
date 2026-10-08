import { z } from 'zod';
import { basename } from 'node:path';
import { createPatch } from 'diff';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import { runBackgroundReindex } from './background-reindex.js';
import type { ServerContext } from '../context.js';
import { resolveSingleNote } from '../resolve/single-note.js';
import { readNoteFile, writeFileAtomic } from '../vault/vault-path.js';
import { allNodeIds } from '../store/nodes.js';
import {
  allBlockIds,
  attachBlockId,
  BLOCK_ID,
  existingBlockId,
  findHeadingLines,
  generateBlockId,
  locateBlock,
} from '../vault/block-id.js';

/**
 * `ensure_block_id` — give a block a stable `^id` so other notes can link
 * or embed it. Idempotent: a block that already carries an id keeps it.
 */
export function registerEnsureBlockIdTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'ensure_block_id',
    'Return the `^block-id` of a block in a note, adding one when it has none, plus a ready `[[Note#^id]]` link. Target the block by `line` or by `heading` (the heading line itself). Placement follows Obsidian: ` ^id` at the end of a paragraph, list item or heading; `^id` on its own line after a table, quote, callout or code block.',
    {
      name: z.string().describe('Path or fuzzy match of the note.'),
      line: z.number().int().positive().optional().describe('1-based line inside the block.'),
      heading: z.string().optional().describe('Heading text; the id goes on that heading line.'),
      id: z.string().optional().describe('Id to write when the block has none: letters, digits, dashes. Default: random 6 characters.'),
      dryRun: z.boolean().optional().describe('If true, return the id and diff without writing.'),
    },
    async (args) => {
      if ((args.line === undefined) === (args.heading === undefined)) {
        throw new Error('Pass exactly one of `line` or `heading`.');
      }
      if (args.id !== undefined && !BLOCK_ID.test(args.id)) {
        throw new Error(`Block id "${args.id}" is invalid: use only letters, digits and dashes.`);
      }

      const note = await readNoteFile(ctx.config.vaultPath, resolveSingleNote(args.name, ctx.db));
      const lines = note.content.split('\n');
      const target = args.line !== undefined
        ? lineTarget(args.line, lines.length, note.rel)
        : headingTarget(lines, args.heading!, note.rel);

      const block = locateBlock(lines, target);
      const link = (id: string): string => `[[${linkTarget(note.rel, ctx)}#^${id}]]`;

      const existing = existingBlockId(lines, block);
      if (existing !== null) {
        return {
          path: note.rel,
          id: existing.id,
          created: false,
          line: existing.line + 1,
          link: link(existing.id),
          ...(args.id !== undefined && args.id !== existing.id ? { requestedIdIgnored: args.id } : {}),
        };
      }

      const taken = allBlockIds(lines);
      if (args.id !== undefined && taken.has(args.id)) {
        throw new Error(`Block id ^${args.id} is already used in "${note.rel}".`);
      }
      const id = args.id ?? generateBlockId(taken);
      const written = attachBlockId(lines, block, id);
      const next = written.lines.join('\n');
      const diff = createPatch(note.rel, note.content, next, 'original', 'proposed');

      if (args.dryRun !== true) {
        await writeFileAtomic(note.abs, next);
        runBackgroundReindex(ctx);
      }
      return {
        path: note.rel,
        id,
        created: true,
        dryRun: args.dryRun === true,
        line: written.line + 1,
        link: link(id),
        diff,
      };
    },
  );
}

/** 0-based index of 1-based `line`, checked against the file length. */
function lineTarget(line: number, lineCount: number, path: string): number {
  if (line > lineCount) throw new Error(`Line ${line} is past the end of "${path}" (${lineCount} lines).`);
  return line - 1;
}

function headingTarget(lines: string[], heading: string, path: string): number {
  const found = findHeadingLines(lines, heading);
  if (found.length === 0) throw new Error(`No heading "${heading}" in "${path}".`);
  if (found.length > 1) {
    throw new Error(
      `Heading "${heading}" appears ${found.length} times in "${path}" (lines ${found.map((i) => i + 1).join(', ')}). Pass \`line\` instead.`,
    );
  }
  return found[0]!;
}

/**
 * The note's link text: its basename, or its path without `.md` when
 * another note in the vault shares the basename.
 */
function linkTarget(path: string, ctx: ServerContext): string {
  const base = basename(path, '.md');
  const clash = allNodeIds(ctx.db).some(
    (id) => id !== path && !id.startsWith('_stub/') && basename(id, '.md') === base,
  );
  return clash ? path.replace(/\.md$/, '') : base;
}
