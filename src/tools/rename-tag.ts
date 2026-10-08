import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import { runBackgroundReindex } from './background-reindex.js';
import type { ServerContext } from '../context.js';
import { listNotePaths, maskCode, readNoteFile, splitFrontmatter, writeNoteFile } from '../vault/scan.js';
import { errorMessage } from '../util/errors.js';

/**
 * `rename_tag` — rename a tag in note bodies and in the `tags` / `tag`
 * frontmatter keys. Tags match case-insensitively and as whole tags, so
 * `#old` never rewrites `#older`; with `includeNested`, `#old/child`
 * becomes `#new/child`. Code, URLs and headings are left alone.
 */
export function registerRenameTagTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'rename_tag',
    'Rename a tag across the vault: inline `#tag` (not in code, URLs or headings) and frontmatter `tags`/`tag`. Dry run by default: returns per-file counts; pass `dryRun: false` to write.',
    {
      from: z.string().min(1).describe('Tag to rename, `#` optional. Case-insensitive.'),
      to: z.string().min(1).describe('New tag name, `#` optional.'),
      includeNested: z.boolean().optional().describe('Also rename `#from/child` to `#to/child`. Default true.'),
      dryRun: z.boolean().optional().describe('Default true. Pass false to write.'),
    },
    async (args) => {
      const from = normalizeTag(args.from);
      const to = normalizeTag(args.to);
      for (const t of [from, to]) {
        if (!isValidTag(t)) throw new Error(`Invalid tag "${t}": use letters, digits, _, - and /, not only digits.`);
      }
      if (from === to) throw new Error('`from` and `to` are the same tag.');
      const dryRun = args.dryRun !== false;
      const nested = args.includeNested !== false;
      const vault = ctx.config.vaultPath;

      const files: Array<{ path: string; inline: number; frontmatter: number }> = [];
      const failed: Array<{ path: string; error: string }> = [];
      for (const path of listNotePaths(ctx.db)) {
        const raw = await readNoteFile(vault, path);
        if (raw === null) continue;
        const res = renameTagInNote(raw, from, to, nested);
        if (res.inline + res.frontmatter === 0) continue;
        if (!dryRun) {
          try {
            await writeNoteFile(vault, path, res.text);
          } catch (err) {
            failed.push({ path, error: errorMessage(err) });
            continue;
          }
        }
        files.push({ path, inline: res.inline, frontmatter: res.frontmatter });
      }

      // Fire-and-forget reindex, once for the whole batch.
      if (!dryRun && files.length > 0) runBackgroundReindex(ctx);

      return {
        dryRun,
        from,
        to,
        filesMatched: files.length,
        replacements: files.reduce((n, f) => n + f.inline + f.frontmatter, 0),
        files,
        ...(failed.length > 0 ? { failed } : {}),
      };
    },
  );
}

export function normalizeTag(tag: string): string {
  return tag.trim().replace(/^#+/, '');
}

export function isValidTag(tag: string): boolean {
  return (
    /^[\p{L}\p{N}_\-/]+$/u.test(tag) &&
    !/^\p{N}+$/u.test(tag) &&
    !tag.startsWith('/') &&
    !tag.endsWith('/') &&
    !tag.includes('//')
  );
}

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** Lookahead that ends a whole tag; `/` continues a nested one. */
function tagEnd(nested: boolean): string {
  return `(?![\\p{L}\\p{N}_\\-${nested ? '' : '/'}])`;
}

export function renameTagInNote(
  raw: string,
  from: string,
  to: string,
  nested: boolean,
): { text: string; inline: number; frontmatter: number } {
  const { frontmatter, body } = splitFrontmatter(raw);
  const fm = renameInFrontmatter(frontmatter, from, to, nested);

  // A tag starts after whitespace or at the start of the body, which rules
  // out URL fragments (`x#old`) and `##` runs; a heading's `# ` has no tag
  // character after the `#`. Matching on the code mask skips code.
  const re = new RegExp(`(?<=^|\\s)#${escapeRegExp(from)}${tagEnd(nested)}`, 'giu');
  const masked = maskCode(body);
  let inline = 0;
  let out = '';
  let at = 0;
  for (const m of masked.matchAll(re)) {
    out += `${body.slice(at, m.index)}#${to}`;
    at = m.index + m[0].length;
    inline++;
  }
  out += body.slice(at);
  return { text: fm.text + out, inline, frontmatter: fm.count };
}

/**
 * Rename the tag inside the `tags` / `tag` frontmatter keys only, line by
 * line, so the rest of the YAML keeps its exact bytes. Handles an inline
 * value (`tags: a, old` or `tags: [a, "#old"]`) and a block list
 * (`tags:` followed by `- old` items).
 */
export function renameInFrontmatter(
  frontmatter: string,
  from: string,
  to: string,
  nested: boolean,
): { text: string; count: number } {
  if (frontmatter === '') return { text: frontmatter, count: 0 };
  const value = new RegExp(`(?<=^|[\\s,\\[\\]"'])(#?)${escapeRegExp(from)}${tagEnd(nested)}`, 'giu');
  let count = 0;
  const rename = (s: string): string =>
    s.replace(value, (_m, hash: string) => {
      count++;
      return `${hash}${to}`;
    });

  let inList = false;
  const lines = frontmatter.split('\n').map((line) => {
    const key = /^(tags?[ \t]*:)(.*)$/i.exec(line);
    if (key) {
      inList = key[2]!.trim() === '';
      return key[1] + rename(key[2]!);
    }
    if (!inList) return line;
    const item = /^([ \t]*-[ \t]+)(.*)$/.exec(line);
    if (item) return item[1] + rename(item[2]!);
    if (!/^[ \t]*(?:#.*)?\r?$/.test(line)) inList = false;
    return line;
  });
  return { text: lines.join('\n'), count };
}
