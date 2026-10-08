import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import { runBackgroundReindex } from './background-reindex.js';
import type { ServerContext } from '../context.js';
import { resolveNodeName } from '../resolve/name-match.js';
import { buildStemLookup, resolveLink } from '../vault/wiki-links.js';
import {
  findHeadings,
  findWikiLinks,
  headingKey,
  listNotePaths,
  readNoteFile,
  splitFrontmatter,
  writeNoteFile,
} from '../vault/scan.js';

/**
 * `rename_heading` — rename one heading in a note and rewrite every link to
 * it: `[[Note#Old]]`, aliased and embedded forms, `[[#Old]]` inside the
 * note itself, and heading paths such as `[[Note#Parent#Old]]`. Embeds make
 * no edges, so every note is scanned rather than only the backlinkers.
 * All files are planned before the first write.
 */
export function registerRenameHeadingTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'rename_heading',
    'Rename a heading in a note and rewrite every wikilink and embed that points to it across the vault. Errors if `from` is missing or `to` already exists in the note.',
    {
      name: z.string().describe('Path or fuzzy match of the note.'),
      from: z.string().min(1).describe('Current heading text, without `#`.'),
      to: z.string().min(1).describe('New heading text, without `#`.'),
      dryRun: z.boolean().optional().describe('If true, report the changes without writing. Default false.'),
    },
    async (args) => {
      const to = args.to.trim();
      if (/[#|^[\]\n]/.test(to)) {
        throw new Error('`to` cannot contain #, |, ^, [, ] or a line break, because links to it would break.');
      }
      const notePath = resolveToSinglePath(args.name, ctx);
      const vault = ctx.config.vaultPath;
      const noteRaw = await readNoteFile(vault, notePath);
      if (noteRaw === null) throw new Error(`Cannot read ${notePath}`);

      const { frontmatter, body } = splitFrontmatter(noteRaw);
      const headings = findHeadings(body);
      const fromKey = headingKey(args.from);
      const hits = headings.filter((h) => h.text.trim() === args.from.trim());
      if (hits.length === 0) throw new Error(`Heading "${args.from}" not found in ${notePath}`);
      if (hits.length > 1) {
        const lines = hits.map((h) => h.line + frontmatter.split('\n').length - 1).join(', ');
        throw new Error(`Heading "${args.from}" appears ${hits.length} times in ${notePath} (lines ${lines}). Rename it by hand.`);
      }
      if (headingKey(to) !== fromKey && headings.some((h) => headingKey(h.text) === headingKey(to))) {
        throw new Error(`Heading "${to}" already exists in ${notePath}`);
      }
      const heading = hits[0]!;
      const renamedNote =
        frontmatter + body.slice(0, heading.start) + to + body.slice(heading.end);

      const allPaths = listNotePaths(ctx.db);
      const allPathsSet = new Set(allPaths);
      const stemLookup = buildStemLookup(allPaths);

      const planned: Array<{ path: string; text: string; links: number }> = [];
      for (const path of allPaths) {
        const raw = path === notePath ? renamedNote : await readNoteFile(vault, path);
        if (raw === null) continue;
        const res = rewriteHeadingLinks(raw, fromKey, to, (target) =>
          target === '' ? path === notePath : resolveLink(target, stemLookup, allPathsSet, path) === notePath,
        );
        if (path === notePath || res.links > 0) planned.push({ path, ...res });
      }

      if (args.dryRun !== true) {
        for (const f of planned) await writeNoteFile(vault, f.path, f.text);
        // Fire-and-forget reindex, once for the whole batch.
        runBackgroundReindex(ctx);
      }

      return {
        ...(args.dryRun === true ? { dryRun: true } : {}),
        path: notePath,
        from: heading.text,
        to,
        filesChanged: planned.map((f) => f.path),
        linksRewritten: planned.reduce((n, f) => n + f.links, 0),
      };
    },
  );
}

/**
 * Rewrite every heading-path segment equal to `fromKey` in links whose note
 * part `pointsAtNote` accepts (`''` is a same-note link). Block references
 * are left alone.
 */
export function rewriteHeadingLinks(
  raw: string,
  fromKey: string,
  to: string,
  pointsAtNote: (target: string) => boolean,
): { text: string; links: number } {
  const { frontmatter, body } = splitFrontmatter(raw);
  let links = 0;
  let out = '';
  let at = 0;
  for (const link of findWikiLinks(body)) {
    if (link.subpath === null || link.subpath.startsWith('^') || !pointsAtNote(link.target)) continue;
    const parts = link.subpath.split('#');
    if (!parts.some((p) => headingKey(p) === fromKey)) continue;
    const subpath = parts.map((p) => (headingKey(p) === fromKey ? to : p)).join('#');
    const inner = link.raw.slice(link.embed ? 3 : 2, -2);
    const hash = inner.indexOf('#');
    const rest = inner.slice(hash + 1 + link.subpath.length);
    out += `${body.slice(at, link.start)}${link.embed ? '!' : ''}[[${inner.slice(0, hash + 1)}${subpath}${rest}]]`;
    at = link.end;
    links++;
  }
  return { text: frontmatter + out + body.slice(at), links };
}

function resolveToSinglePath(name: string, ctx: ServerContext): string {
  const matches = resolveNodeName(name, ctx.db);
  if (matches.length === 0) {
    throw new Error(`No note found matching "${name}"`);
  }
  const first = matches[0]!;
  const ambiguous =
    matches.length > 1 &&
    (first.matchType === 'substring' ||
      first.matchType === 'case-insensitive' ||
      first.matchType === 'alias');
  if (ambiguous) {
    const candidates = matches
      .slice(0, 10)
      .map((m) => `- ${m.title} (${m.nodeId})`)
      .join('\n');
    throw new Error(
      `Multiple notes match "${name}". Please be more specific. Candidates:\n${candidates}`,
    );
  }
  return first.nodeId;
}
