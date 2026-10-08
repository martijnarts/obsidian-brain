import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import { runBackgroundReindex } from './background-reindex.js';
import type { ServerContext } from '../context.js';
import {
  compileUserRegex,
  fencedCodeRanges,
  lineAt,
  listNotePaths,
  readNoteFile,
  splitFrontmatter,
  writeNoteFile,
} from '../vault/scan.js';
import { errorMessage } from '../util/errors.js';

const SAMPLES_PER_FILE = 3;
const SAMPLE_CONTEXT = 80;

/** One match: `[start, end)` in the raw file and the text that replaces it. */
interface Hit {
  start: number;
  end: number;
  text: string;
}

interface Sample {
  line: number;
  before: string;
  after: string;
}

/**
 * `search_and_replace` — vault-wide find/replace in note bodies. Dry run by
 * default. Frontmatter and fenced code blocks are left alone unless the
 * caller opts in. Every file is planned before any is written, so the
 * `maxFiles` cap refuses a too-wide replace without touching the vault.
 */
export function registerSearchAndReplaceTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'search_and_replace',
    'Find and replace text across note bodies. Dry run by default: returns per-file match counts and before/after samples; pass `dryRun: false` to write. Skips frontmatter and fenced code unless asked.',
    {
      pattern: z.string().min(1).describe('Text to find, or a JavaScript regex when `regex` is true. `^`/`$` match at line ends.'),
      replacement: z.string().describe('Replacement text. With `regex`, `$1`, `$<name>` and `$&` expand.'),
      regex: z.boolean().optional().describe('Treat `pattern` as a regex. Default false.'),
      caseSensitive: z.boolean().optional().describe('Default true.'),
      folder: z.string().optional().describe('Only touch notes under this folder.'),
      includeFrontmatter: z.boolean().optional().describe('Also replace inside YAML frontmatter. Default false.'),
      includeCode: z.boolean().optional().describe('Also replace inside fenced code blocks. Default false.'),
      dryRun: z.boolean().optional().describe('Default true. Pass false to write.'),
      maxFiles: z.number().int().positive().optional().describe('Refuse to write when more files match. Default 200.'),
    },
    async (args) => {
      const dryRun = args.dryRun !== false;
      const maxFiles = args.maxFiles ?? 200;
      const re = compileUserRegex(args.pattern, args.caseSensitive === false ? 'gmi' : 'gm', args.regex !== true);
      const vault = ctx.config.vaultPath;

      const planned: Array<{ path: string; raw: string; hits: Hit[] }> = [];
      for (const path of listNotePaths(ctx.db, args.folder)) {
        const raw = await readNoteFile(vault, path);
        if (raw === null) continue;
        const hits = findHits(raw, re, args.replacement, args.regex === true, {
          includeFrontmatter: args.includeFrontmatter === true,
          includeCode: args.includeCode === true,
        });
        if (hits.length > 0) planned.push({ path, raw, hits });
      }

      const filesMatched = planned.length;
      const totalMatches = planned.reduce((n, f) => n + f.hits.length, 0);
      if (!dryRun && filesMatched > maxFiles) {
        throw new Error(
          `${filesMatched} files match, more than maxFiles (${maxFiles}). Nothing was written. Narrow \`folder\` or raise \`maxFiles\`.`,
        );
      }

      if (dryRun) {
        return {
          dryRun: true,
          filesMatched,
          totalMatches,
          ...(filesMatched > maxFiles ? { truncated: true } : {}),
          files: planned.slice(0, maxFiles).map((f) => ({
            path: f.path,
            matches: f.hits.length,
            samples: samplesOf(f.raw, f.hits),
          })),
        };
      }

      const failed: Array<{ path: string; error: string }> = [];
      const files: Array<{ path: string; matches: number }> = [];
      for (const f of planned) {
        try {
          await writeNoteFile(vault, f.path, applyHits(f.raw, f.hits));
          files.push({ path: f.path, matches: f.hits.length });
        } catch (err) {
          failed.push({ path: f.path, error: errorMessage(err) });
        }
      }

      // Fire-and-forget reindex, once for the whole batch.
      if (files.length > 0) runBackgroundReindex(ctx);

      return {
        dryRun: false,
        filesChanged: files.length,
        totalMatches: files.reduce((n, f) => n + f.matches, 0),
        files,
        ...(failed.length > 0 ? { failed } : {}),
      };
    },
  );
}

/**
 * Every match in the editable parts of `raw` with its replacement text.
 * Each editable segment is matched on its own, so a match never spans
 * frontmatter or a code fence.
 */
export function findHits(
  raw: string,
  re: RegExp,
  replacement: string,
  expand: boolean,
  opts: { includeFrontmatter: boolean; includeCode: boolean },
): Hit[] {
  const regionStart = opts.includeFrontmatter ? 0 : splitFrontmatter(raw).frontmatter.length;
  const region = raw.slice(regionStart);
  const skip = opts.includeCode ? [] : fencedCodeRanges(region);

  const segments: Array<[number, number]> = [];
  let at = 0;
  for (const [s, e] of skip) {
    if (s > at) segments.push([at, s]);
    at = e;
  }
  if (at < region.length) segments.push([at, region.length]);

  const hits: Hit[] = [];
  for (const [s, e] of segments) {
    const segment = region.slice(s, e);
    for (const m of segment.matchAll(re)) {
      const start = regionStart + s + m.index;
      hits.push({
        start,
        end: start + m[0].length,
        text: expand ? expandTemplate(replacement, m, segment) : replacement,
      });
    }
  }
  return hits;
}

/** `String.prototype.replace` substitution rules for one match. */
export function expandTemplate(template: string, m: RegExpMatchArray, input: string): string {
  const groups = m.length - 1;
  return template.replace(/\$(\$|&|`|'|<([^>]*)>|(\d\d?))/g, (token, kind: string, name?: string, num?: string) => {
    if (kind === '$') return '$';
    if (kind === '&') return m[0];
    if (kind === '`') return input.slice(0, m.index);
    if (kind === "'") return input.slice(m.index! + m[0].length);
    if (name !== undefined) return m.groups ? (m.groups[name] ?? '') : token;
    const n = Number(num);
    if (n >= 1 && n <= groups) return m[n] ?? '';
    // `$12` with fewer than 12 groups reads as `$1` followed by `2`.
    const first = Number(num![0]);
    if (num!.length === 2 && first >= 1 && first <= groups) return (m[first] ?? '') + num![1];
    return token;
  });
}

function applyHits(raw: string, hits: Hit[]): string {
  let out = '';
  let at = 0;
  for (const h of hits) {
    out += raw.slice(at, h.start) + h.text;
    at = h.end;
  }
  return out + raw.slice(at);
}

/** Before/after excerpts for the first few lines that hold a match. */
function samplesOf(raw: string, hits: Hit[]): Sample[] {
  const byLine = new Map<number, Hit[]>();
  for (const h of hits) {
    const line = lineAt(raw, h.start);
    if (!byLine.has(line) && byLine.size >= SAMPLES_PER_FILE) break;
    byLine.set(line, [...(byLine.get(line) ?? []), h]);
  }
  return [...byLine].map(([line, group]) => {
    const first = group[0]!;
    const last = group[group.length - 1]!;
    const lineStart = raw.lastIndexOf('\n', first.start - 1) + 1;
    const nl = raw.indexOf('\n', last.end);
    const lineEnd = nl === -1 ? raw.length : nl;
    const from = Math.max(lineStart, first.start - SAMPLE_CONTEXT);
    const to = Math.min(lineEnd, last.end + SAMPLE_CONTEXT);
    const shifted = group.map((h) => ({ ...h, start: h.start - from, end: h.end - from }));
    const before = raw.slice(from, to);
    return { line, before, after: applyHits(before, shifted) };
  });
}
