import { z } from 'zod';
import { readFile, readdir, stat } from 'node:fs/promises';
import matter from 'gray-matter';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';
import { resolveVaultPath } from '../vault/vault-path.js';
import { countTasks, extractHeadings } from '../vault/note-parts.js';
import { extractInlineTags } from '../vault/parser.js';
import { getNode } from '../store/nodes.js';
import { countEdgesByTarget, getEdgesBySource } from '../store/edges.js';

/**
 * `file_info` — describe a vault path without returning its body. Sizes and
 * times come from the file system; heading, task and tag counts from the
 * note on disk; link counts from the index, so they lag a fresh write until
 * the background reindex catches up.
 */
export function registerFileInfoTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'file_info',
    'Describe a vault path without its body: kind (note, attachment, folder), size, times, and for a note its link, heading, task and tag counts. Link counts come from the index.',
    {
      path: z.string().describe('Vault-relative path of a note, attachment or folder.'),
    },
    async (args) => {
      const { rel, abs } = resolveVaultPath(ctx.config.vaultPath, args.path);
      const st = await stat(abs).catch(() => {
        throw new Error(`Path not found: ${args.path}`);
      });
      const times = {
        mtime: st.mtime.toISOString(),
        // birthtime is 0 on file systems that do not record it.
        ctime: (st.birthtimeMs > 0 ? st.birthtime : st.ctime).toISOString(),
      };

      if (st.isDirectory()) {
        const entries = await readdir(abs);
        return { path: rel, kind: 'folder', children: entries.length, ...times };
      }
      if (!rel.toLowerCase().endsWith('.md')) {
        return { path: rel, kind: 'attachment', size: st.size, ...times };
      }

      const raw = await readFile(abs, 'utf-8');
      let fm: Record<string, unknown> = {};
      let body = raw;
      try {
        const parsed = matter(raw);
        fm = parsed.data as Record<string, unknown>;
        body = parsed.content;
      } catch {
        // Malformed frontmatter: count the whole file as body, as the parser does.
      }

      const outgoing = getEdgesBySource(ctx.db, rel);
      return {
        path: rel,
        kind: 'note',
        size: st.size,
        ...times,
        indexed: getNode(ctx.db, rel) !== undefined,
        links: {
          outgoing: outgoing.length,
          incoming: countEdgesByTarget(ctx.db, rel),
          unresolved: outgoing.filter((e) => e.targetId.startsWith('_stub/')).length,
        },
        headings: extractHeadings(raw).length,
        tasks: countTasks(raw),
        tags: collectTags(fm, body),
      };
    },
  );
}

function collectTags(fm: Record<string, unknown>, body: string): string[] {
  const tags = new Set<string>();
  for (const key of ['tags', 'tag']) {
    const value = fm[key];
    const list = Array.isArray(value)
      ? value
      : typeof value === 'string'
        ? value.split(/[,\s]+/)
        : [];
    for (const t of list) {
      if (typeof t !== 'string') continue;
      const tag = t.trim().replace(/^#/, '');
      if (tag !== '') tags.add(tag);
    }
  }
  for (const t of extractInlineTags(body)) tags.add(t);
  return [...tags];
}
