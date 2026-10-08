import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';
import { similarity } from '../vault/fuzzy.js';
import { inFolder, normalizeFolder } from '../vault/vault-path.js';

type MatchedOn = 'name' | 'title' | 'alias';

interface Hit {
  path: string;
  title: string;
  matchedOn: MatchedOn;
  alias?: string;
  score: number;
}

/**
 * Score how well `query` names `candidate`, in [0, 1]. Tiers, best first:
 * exact, prefix, substring, every query word present, characters in order
 * (quick-switcher style), then a Levenshtein typo match. Both sides are
 * compared lower-cased. 0 means no match.
 */
export function scoreName(query: string, candidate: string): number {
  const q = query.trim().toLowerCase();
  const c = candidate.toLowerCase();
  if (q === '' || c === '') return 0;
  if (c === q) return 1;
  if (c.startsWith(q)) return 0.9;
  const idx = c.indexOf(q);
  if (idx !== -1) return /[\s\-_/.]/.test(c.charAt(idx - 1)) ? 0.85 : 0.8;
  const words = q.split(/\s+/);
  if (words.length > 1 && words.every((w) => c.includes(w))) return 0.7;

  let best = 0;
  const span = subsequenceSpan(q, c);
  // A tight span (characters close together) scores near 0.6, a loose one near 0.4.
  if (span !== null) best = 0.4 + 0.2 * (q.length / span);
  const sim = similarity(q, c);
  if (sim >= 0.6) best = Math.max(best, sim * 0.6);
  return best;
}

/** Length of the shortest prefix-anchored window of `c` holding `q`'s characters in order. */
function subsequenceSpan(q: string, c: string): number | null {
  let start = -1;
  let pos = 0;
  for (const ch of q) {
    if (ch === ' ') continue;
    const found = c.indexOf(ch, pos);
    if (found === -1) return null;
    if (start === -1) start = found;
    pos = found + 1;
  }
  return start === -1 ? null : pos - start;
}

/** Frontmatter `aliases` (or legacy `alias`) as a list; a string is split on commas. */
export function noteAliases(fm: Record<string, unknown>): string[] {
  const out: string[] = [];
  for (const raw of [fm.aliases, fm.alias]) {
    const list = Array.isArray(raw) ? raw : typeof raw === 'string' ? raw.split(',') : [];
    for (const a of list) {
      if (typeof a === 'string' && a.trim() !== '') out.push(a.trim());
    }
  }
  return out;
}

export function registerFindNotesByNameTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'find_notes_by_name',
    "Find notes by a half-remembered name, like Obsidian's quick switcher: fuzzy, case-insensitive match against the filename, title and frontmatter `aliases`, best first. Use `grep_vault` or `search` to look inside note bodies instead.",
    {
      query: z.string().min(1).describe('Name fragment. Typos and non-contiguous characters still match.'),
      folder: z.string().optional().describe('Only notes under this vault-relative folder.'),
      limit: z.number().int().min(1).max(100).optional().describe('Max results (1-100). Default 20.'),
    },
    async (args) => {
      const folder = normalizeFolder(args.folder);
      const limit = args.limit ?? 20;
      const rows = (
        ctx.db.prepare('SELECT id, title, frontmatter FROM nodes').all() as Array<{
          id: string;
          title: string;
          frontmatter: string;
        }>
      ).filter((r) => !r.id.startsWith('_stub/'));
      // A typo in the folder reads as an error, not as zero results.
      if (folder !== '' && !rows.some((r) => inFolder(r.id, folder))) {
        throw new Error(`Folder not found: "${args.folder}"`);
      }

      const hits: Hit[] = [];
      for (const row of rows) {
        if (!inFolder(row.id, folder)) continue;
        const fm = JSON.parse(row.frontmatter) as Record<string, unknown>;
        if (fm._stub === true) continue;
        const stem = (row.id.split('/').pop() ?? row.id).replace(/\.md$/, '');

        // Strict `>` keeps the earlier field on a tie: name, then title, then alias.
        let best: Hit | undefined;
        const consider = (text: string, matchedOn: MatchedOn, alias?: string): void => {
          const score = scoreName(args.query, text);
          if (score > 0 && (best === undefined || score > best.score)) {
            best = { path: row.id, title: row.title, matchedOn, score, ...(alias ? { alias } : {}) };
          }
        };
        consider(stem, 'name');
        consider(row.title, 'title');
        for (const alias of noteAliases(fm)) consider(alias, 'alias', alias);
        if (best !== undefined) hits.push(best);
      }

      hits.sort((a, b) => b.score - a.score || a.path.length - b.path.length || a.path.localeCompare(b.path));
      const results = hits.slice(0, limit).map((h) => ({ ...h, score: Math.round(h.score * 1000) / 1000 }));
      return {
        total: hits.length,
        ...(hits.length > limit ? { truncated: true } : {}),
        results,
      };
    },
  );
}
