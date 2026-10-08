import { readFile } from 'fs/promises';
import { join } from 'path';
import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';
import { collectMarkdownFiles } from '../vault/parser.js';
import { resolveFolder } from '../vault/vault-path.js';
import { compileUserRegex } from '../util/safe-regex.js';

/** Wall-clock budget for one scan. Past it the tool returns what it has. */
export const GREP_TIME_BUDGET_MS = 2_000;
/** Returned lines are clipped to this many characters (around the match). */
const MAX_LINE_CHARS = 400;

interface LineMatch {
  line: number;
  text: string;
  before?: string[];
  after?: string[];
}

interface FileMatches {
  path: string;
  matches: LineMatch[];
  moreMatches?: true;
}

/** Clip a long line to a window that keeps the character at `at` visible. */
function clip(line: string, at = 0): string {
  if (line.length <= MAX_LINE_CHARS) return line;
  const start = Math.max(0, Math.min(at - MAX_LINE_CHARS / 4, line.length - MAX_LINE_CHARS));
  const body = line.slice(start, start + MAX_LINE_CHARS);
  return (start > 0 ? '…' : '') + body + (start + MAX_LINE_CHARS < line.length ? '…' : '');
}

export function registerGrepVaultTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'grep_vault',
    'Search the text of every note file on disk, frontmatter included, for a literal string or a JavaScript regex, line by line like grep. Returns matching lines with 1-based line numbers and surrounding lines; `truncated: true` means the scan stopped early at the file limit or the 2 s time cap.',
    {
      query: z.string().min(1).describe('Literal text, or a regex source without slashes when `regex` is true.'),
      regex: z.boolean().optional().describe('Treat `query` as a JavaScript regex. Default false.'),
      caseSensitive: z.boolean().optional().describe('Default false.'),
      folder: z.string().optional().describe('Only notes under this vault-relative folder.'),
      contextLines: z.number().int().min(0).max(10).optional().describe('Lines of context before and after each match. Default 1.'),
      limit: z.number().int().min(1).max(200).optional().describe('Max files with matches to return. Default 20.'),
      maxMatchesPerFile: z.number().int().min(1).max(100).optional().describe('Max matching lines per file. Default 5.'),
    },
    async (args) => {
      const pattern = compileUserRegex(args.query, args.caseSensitive === true ? '' : 'i', args.regex !== true);
      const contextLines = args.contextLines ?? 1;
      const limit = args.limit ?? 20;
      const maxMatches = args.maxMatchesPerFile ?? 5;

      const root = ctx.config.vaultPath;
      const folder = await resolveFolder(root, args.folder);
      const paths = (await collectMarkdownFiles(root, folder.rel)).sort();

      const deadline = Date.now() + GREP_TIME_BUDGET_MS;
      const files: FileMatches[] = [];
      let scanned = 0;
      let stoppedBy: 'limit' | 'time' | undefined;

      scan: for (const relPath of paths) {
        if (files.length >= limit) {
          stoppedBy = 'limit';
          break;
        }
        if (Date.now() > deadline) {
          stoppedBy = 'time';
          break;
        }
        let raw: string;
        try {
          raw = await readFile(join(root, relPath), 'utf-8');
        } catch {
          continue; // deleted or unreadable since the directory walk
        }
        const lines = raw.split(/\r?\n/);
        const matches: LineMatch[] = [];
        let more = false;
        for (let i = 0; i < lines.length; i++) {
          if ((i & 255) === 255 && Date.now() > deadline) {
            if (matches.length > 0) files.push({ path: relPath, matches });
            stoppedBy = 'time';
            break scan;
          }
          const line = lines[i]!;
          const m = pattern.exec(line);
          if (m === null) continue;
          if (matches.length >= maxMatches) {
            more = true;
            break;
          }
          const hit: LineMatch = { line: i + 1, text: clip(line, m.index) };
          if (contextLines > 0) {
            hit.before = lines.slice(Math.max(0, i - contextLines), i).map((l) => clip(l));
            hit.after = lines.slice(i + 1, i + 1 + contextLines).map((l) => clip(l));
          }
          matches.push(hit);
        }
        scanned++;
        if (matches.length > 0) {
          files.push(more ? { path: relPath, matches, moreMatches: true } : { path: relPath, matches });
        }
      }

      return {
        files,
        scannedFiles: scanned,
        totalFiles: paths.length,
        ...(stoppedBy !== undefined ? { truncated: true, stoppedBy } : {}),
      };
    },
  );
}
