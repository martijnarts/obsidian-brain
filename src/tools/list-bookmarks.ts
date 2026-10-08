import { z } from 'zod';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';

const BOOKMARK_TYPES = ['file', 'folder', 'search', 'heading', 'block', 'group'] as const;
type BookmarkType = (typeof BOOKMARK_TYPES)[number];

interface Bookmark {
  type: BookmarkType;
  title?: string;
  path?: string;
  query?: string;
  subpath?: string;
  items?: Bookmark[];
}

/** Keeps the known fields of one raw entry; drops entries of unknown type. */
function toBookmark(raw: unknown): Bookmark | null {
  if (raw === null || typeof raw !== 'object') return null;
  const r = raw as Record<string, unknown>;
  if (!BOOKMARK_TYPES.includes(r.type as BookmarkType)) return null;
  const b: Bookmark = { type: r.type as BookmarkType };
  for (const key of ['title', 'path', 'query', 'subpath'] as const) {
    if (typeof r[key] === 'string') b[key] = r[key];
  }
  if (b.type === 'group') b.items = toBookmarks(r.items);
  return b;
}

function toBookmarks(raw: unknown): Bookmark[] {
  if (!Array.isArray(raw)) return [];
  return raw.map(toBookmark).filter((b): b is Bookmark => b !== null);
}

/** Without `group` in `types`, a group's matching children move up a level. */
function filterBookmarks(items: Bookmark[], types: Set<BookmarkType>): Bookmark[] {
  const out: Bookmark[] = [];
  for (const b of items) {
    if (b.type === 'group') {
      const children = filterBookmarks(b.items ?? [], types);
      if (types.has('group')) out.push({ ...b, items: children });
      else out.push(...children);
    } else if (types.has(b.type)) {
      out.push(b);
    }
  }
  return out;
}

function countBookmarks(items: Bookmark[]): number {
  return items.reduce((n, b) => n + (b.type === 'group' ? countBookmarks(b.items ?? []) : 1), 0);
}

export function registerListBookmarksTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'list_bookmarks',
    "List the vault's Obsidian bookmarks (`.obsidian/bookmarks.json`) as a tree of groups, files, folders, searches, headings and blocks. `totalItems` counts every non-group bookmark.",
    {
      types: z.array(z.enum(BOOKMARK_TYPES)).optional().describe('Only these bookmark types. Without `group`, group contents are flattened.'),
    },
    async (args) => {
      let raw: string;
      try {
        raw = await readFile(join(ctx.config.vaultPath, '.obsidian', 'bookmarks.json'), 'utf-8');
      } catch {
        return { totalItems: 0, items: [], note: 'No bookmarks file in this vault.' };
      }

      let items: Bookmark[];
      try {
        const parsed = JSON.parse(raw) as { items?: unknown };
        if (!Array.isArray(parsed?.items)) throw new Error('no items array');
        items = toBookmarks(parsed.items);
      } catch {
        return { totalItems: 0, items: [], note: 'The bookmarks file is not valid bookmarks JSON.' };
      }

      if (args.types && args.types.length > 0) items = filterBookmarks(items, new Set(args.types));
      return { totalItems: countBookmarks(items), items };
    },
  );
}
