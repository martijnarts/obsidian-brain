import { stat } from 'fs/promises';
import { join } from 'path';
import jsonLogic from 'json-logic-js';
import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';

/** The record a `filter` sees for each note. */
export interface NoteRecord {
  path: string;
  folder: string;
  name: string;
  title: string;
  tags: string[];
  frontmatter: Record<string, unknown>;
  mtime: string;
  size: number;
}

/**
 * `has_tag(tags, tag)`: case-insensitive, ignores a leading `#`, and a
 * parent tag matches its nested tags (`area` matches `area/work`), as in
 * Obsidian's tag search.
 */
jsonLogic.add_operation('has_tag', (tags: unknown, tag: unknown): boolean => {
  if (!Array.isArray(tags) || typeof tag !== 'string') return false;
  const want = tag.replace(/^#/, '').toLowerCase();
  return tags.some((t) => {
    if (typeof t !== 'string') return false;
    const have = t.toLowerCase();
    return have === want || have.startsWith(want + '/');
  });
});

/**
 * Frontmatter `tags` (or `tag`) plus the body's inline tags, without `#`,
 * deduplicated case-insensitively. A frontmatter string is split on commas
 * and whitespace, as Obsidian reads it.
 */
export function noteTags(fm: Record<string, unknown>): string[] {
  const raw: unknown[] = [];
  for (const v of [fm.tags, fm.tag, fm.inline_tags]) {
    if (Array.isArray(v)) raw.push(...v);
    else if (typeof v === 'string') raw.push(...v.split(/[,\s]+/));
  }
  const seen = new Set<string>();
  const out: string[] = [];
  for (const t of raw) {
    if (typeof t !== 'string') continue;
    const tag = t.trim().replace(/^#/, '');
    if (tag === '' || seen.has(tag.toLowerCase())) continue;
    seen.add(tag.toLowerCase());
    out.push(tag);
  }
  return out;
}

function sortValue(rec: NoteRecord, field: string): unknown {
  if (field === 'path' || field === 'title' || field === 'mtime') return rec[field];
  return rec.frontmatter[field.slice('frontmatter.'.length)];
}

/** Numbers compare numerically, anything else as text. */
function compareValues(a: unknown, b: unknown): number {
  if (typeof a === 'number' && typeof b === 'number') return a - b;
  return String(a).localeCompare(String(b));
}

export function registerQueryNotesTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'query_notes',
    'Filter notes by metadata with a JsonLogic `filter`; the headless replacement for Dataview queries. Each note is `{path, folder, name, title, tags, frontmatter, mtime, size}`: `tags` merges frontmatter and inline tags without `#`, `mtime` is ISO 8601, and `{"var": "frontmatter.status"}` reads a property. The extra operator `has_tag` matches a tag case-insensitively, nested tags included. Example: `{"and": [{"has_tag": [{"var": "tags"}, "book"]}, {">=": [{"var": "frontmatter.rating"}, 4]}]}`.',
    {
      filter: z.record(z.string(), z.unknown()).describe('JsonLogic expression; a note matches when it is truthy. `{"==": [1, 1]}` matches every note.'),
      fields: z.array(z.string()).optional().describe('Frontmatter keys to return. Default all.'),
      sort: z
        .object({
          field: z.string().describe('`path`, `title`, `mtime` or `frontmatter.<key>`.'),
          order: z.enum(['asc', 'desc']).optional().describe('Default `asc`.'),
        })
        .optional()
        .describe('Default: by path, ascending.'),
      limit: z.number().int().min(1).max(500).optional().describe('Max results (1-500). Default 50.'),
    },
    async (args) => {
      const { filter, fields } = args;
      if (filter === null || typeof filter !== 'object' || Array.isArray(filter) || !jsonLogic.is_logic(filter)) {
        throw new Error(
          'filter must be a JsonLogic object with exactly one operator, e.g. {"==": [{"var": "frontmatter.status"}, "active"]}',
        );
      }
      const sortField = args.sort?.field ?? 'path';
      if (!['path', 'title', 'mtime'].includes(sortField) && !/^frontmatter\..+/.test(sortField)) {
        throw new Error(`sort.field must be path, title, mtime or frontmatter.<key>, got "${sortField}"`);
      }
      const desc = args.sort?.order === 'desc';
      const limit = args.limit ?? 50;

      const rows = (
        ctx.db.prepare('SELECT id, title, frontmatter FROM nodes').all() as Array<{
          id: string;
          title: string;
          frontmatter: string;
        }>
      ).filter((r) => !r.id.startsWith('_stub/'));

      const matched: NoteRecord[] = [];
      for (const row of rows) {
        const parsed = JSON.parse(row.frontmatter) as Record<string, unknown>;
        if (parsed._stub === true) continue;
        let st;
        try {
          st = await stat(join(ctx.config.vaultPath, row.id));
        } catch {
          continue; // deleted on disk, index not caught up yet
        }
        const { inline_tags: _inline, ...frontmatter } = parsed;
        const slash = row.id.lastIndexOf('/');
        const rec: NoteRecord = {
          path: row.id,
          folder: slash === -1 ? '' : row.id.slice(0, slash),
          name: row.id.slice(slash + 1).replace(/\.md$/, ''),
          title: row.title,
          tags: noteTags(parsed),
          frontmatter,
          mtime: st.mtime.toISOString(),
          size: st.size,
        };
        let hit: unknown;
        try {
          hit = jsonLogic.apply(filter as Parameters<typeof jsonLogic.apply>[0], rec);
        } catch (err) {
          throw new Error(`Invalid filter: ${(err as Error).message}`);
        }
        if (jsonLogic.truthy(hit)) matched.push(rec);
      }

      matched.sort((a, b) => {
        const va = sortValue(a, sortField);
        const vb = sortValue(b, sortField);
        const missingA = va === undefined || va === null;
        const missingB = vb === undefined || vb === null;
        if (missingA || missingB) return missingA === missingB ? a.path.localeCompare(b.path) : missingA ? 1 : -1;
        const c = compareValues(va, vb);
        return (desc ? -c : c) || a.path.localeCompare(b.path);
      });

      const results = matched.slice(0, limit).map((rec) => ({
        path: rec.path,
        title: rec.title,
        tags: rec.tags,
        mtime: rec.mtime,
        size: rec.size,
        frontmatter: fields
          ? Object.fromEntries(fields.filter((k) => k in rec.frontmatter).map((k) => [k, rec.frontmatter[k]]))
          : rec.frontmatter,
      }));
      return {
        total: matched.length,
        ...(matched.length > limit ? { truncated: true } : {}),
        results,
      };
    },
  );
}
