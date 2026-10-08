import { promises as fs } from 'fs';
import { sep } from 'path';
import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';
import { buildStemLookup, resolveLink } from '../vault/wiki-links.js';
import {
  findBlockIds,
  findHeadings,
  findWikiLinks,
  headingKey,
  lineAt,
  listNotePaths,
  readNoteFile,
  splitFrontmatter,
} from '../vault/scan.js';

type Reason = 'note_not_found' | 'heading_not_found' | 'block_not_found';

interface BrokenLink {
  source: string;
  line: number;
  link: string;
  reason: Reason;
}

interface Anchors {
  headings: Set<string>;
  blocks: Set<string>;
}

/**
 * `find_broken_links` — scan note bodies for `[[wikilinks]]` that do not
 * resolve: the target note is missing, or the `#heading` / `#^block`
 * subpath is missing from a target that exists. Resolution reuses the
 * parser's `resolveLink`, so a link reported here is a link the index
 * stores as a `_stub/` edge (or a subpath the target lacks).
 */
export function registerFindBrokenLinksTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'find_broken_links',
    'List wikilinks whose target note is missing, or whose `#heading` / `#^block` is missing from the target. Read-only. Returns source path, 1-based line, link text and reason per broken link.',
    {
      folder: z.string().optional().describe('Only scan notes under this folder.'),
      excludeFolders: z.array(z.string()).optional().describe('Folders to skip.'),
      limit: z.number().int().positive().optional().describe('Max broken links returned. Default 100.'),
    },
    async (args) => {
      const vault = ctx.config.vaultPath;
      const allPaths = listNotePaths(ctx.db);
      const allPathsSet = new Set(allPaths);
      const stemLookup = buildStemLookup(allPaths);
      const sources = listNotePaths(ctx.db, args.folder, args.excludeFolders);
      const anchorCache = new Map<string, Anchors | null>();
      let attachments: Set<string> | undefined;

      const isAttachment = async (target: string): Promise<boolean> => {
        if (!/\.[A-Za-z0-9]+$/.test(target) || target.endsWith('.md')) return false;
        attachments ??= await listAttachments(vault);
        return attachments.has(target) || attachments.has(target.split('/').pop()!);
      };

      const anchorsOf = async (path: string, raw?: string): Promise<Anchors | null> => {
        if (!anchorCache.has(path)) {
          const text = raw ?? (await readNoteFile(vault, path));
          const body = text === null ? null : splitFrontmatter(text).body;
          anchorCache.set(
            path,
            body === null
              ? null
              : {
                  headings: new Set(findHeadings(body).map((h) => headingKey(h.text))),
                  blocks: findBlockIds(body),
                },
          );
        }
        return anchorCache.get(path)!;
      };

      const broken: BrokenLink[] = [];
      let scannedFiles = 0;
      for (const source of sources) {
        const raw = await readNoteFile(vault, source);
        if (raw === null) continue;
        scannedFiles++;
        const { frontmatter, body } = splitFrontmatter(raw);
        for (const link of findWikiLinks(body)) {
          if (link.embed) continue;
          const report = (reason: Reason): void => {
            broken.push({ source, line: lineAt(raw, frontmatter.length + link.start), link: link.raw, reason });
          };

          let target: string | null;
          if (link.target === '') {
            target = source;
          } else {
            target = resolveLink(link.target, stemLookup, allPathsSet, source);
            if (target === null && !(await isAttachment(link.target))) {
              report('note_not_found');
              continue;
            }
          }
          if (target === null || link.subpath === null || link.subpath.trim() === '') continue;

          const anchors = await anchorsOf(target, target === source ? raw : undefined);
          if (anchors === null) continue;
          const parts = link.subpath.split('#').map((p) => p.trim()).filter((p) => p !== '');
          const last = parts[parts.length - 1];
          if (last === undefined) continue;
          if (last.startsWith('^')) {
            if (!anchors.blocks.has(last.slice(1))) report('block_not_found');
          } else if (parts.some((p) => !anchors.headings.has(headingKey(p)))) {
            report('heading_not_found');
          }
        }
      }

      const cap = args.limit ?? 100;
      return {
        scannedFiles,
        total: broken.length,
        ...(broken.length > cap ? { truncated: true } : {}),
        brokenLinks: broken.slice(0, cap),
      };
    },
  );
}

/**
 * Every non-markdown file in the vault, by vault-relative path and by
 * basename, skipping hidden folders. A wikilink to one of these (e.g.
 * `[[report.pdf]]`) is an attachment link, not a broken note link.
 */
async function listAttachments(vaultPath: string): Promise<Set<string>> {
  const out = new Set<string>();
  for (const rel of await fs.readdir(vaultPath, { recursive: true })) {
    const parts = rel.split(sep);
    if (rel.endsWith('.md') || parts.some((p) => p.startsWith('.'))) continue;
    out.add(parts.join('/'));
    out.add(parts[parts.length - 1]!);
  }
  return out;
}
