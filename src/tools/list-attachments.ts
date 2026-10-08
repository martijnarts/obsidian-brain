import { z } from 'zod';
import { readdir, stat } from 'node:fs/promises';
import { join, posix } from 'node:path';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';
import { resolveVaultPath } from '../vault/vault-path.js';

interface Attachment {
  path: string;
  extension: string;
  size: number;
  references: number;
}

/**
 * `list_attachments` — every non-markdown file in the vault, with how many
 * notes point at it. The index stores only `[[wikilink]]` edges, so the
 * count comes from scanning note bodies in the `nodes` table for embeds and
 * markdown links too. A link target matches an attachment the way Obsidian
 * resolves it: by vault-relative path, by path relative to the note, or by
 * bare filename (or a path suffix) anywhere in the vault.
 */
export function registerListAttachmentsTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'list_attachments',
    'List non-markdown files (images, PDFs, ...) with their size and `references`: how many notes link to or embed each one. `unreferencedOnly` finds orphans.',
    {
      folder: z.string().optional().describe('Vault-relative folder to list, recursively. Default: the whole vault.'),
      extensions: z.array(z.string()).optional().describe('Keep only these extensions, case-insensitive, e.g. `["png", ".pdf"]`.'),
      unreferencedOnly: z.boolean().optional().describe('Return only attachments no note references.'),
      limit: z.number().int().positive().optional().describe('Max results. Default 100.'),
      offset: z.number().int().min(0).optional().describe('Results to skip, for paging. Default 0.'),
    },
    async (args) => {
      const limit = args.limit ?? 100;
      const offset = args.offset ?? 0;

      let scope = '';
      if (args.folder !== undefined) {
        const { rel, abs } = resolveVaultPath(ctx.config.vaultPath, args.folder);
        const st = await stat(abs).catch(() => undefined);
        if (!st?.isDirectory()) throw new Error(`Folder not found: ${args.folder}`);
        scope = rel;
      }

      // References resolve against every attachment in the vault, not just
      // the listed folder: a bare filename may match a file elsewhere.
      const all = await collectAttachments(ctx.config.vaultPath);
      const counts = countReferences(ctx, all.map((a) => a.path));

      const wanted = args.extensions?.map((e) => e.toLowerCase().replace(/^\./, ''));
      const listed: Attachment[] = all
        .filter((a) => scope === '' || a.path.startsWith(`${scope}/`))
        .filter((a) => wanted === undefined || wanted.includes(a.extension))
        .map((a) => ({ ...a, references: counts.get(a.path) ?? 0 }))
        .filter((a) => args.unreferencedOnly !== true || a.references === 0);

      return {
        total: listed.length,
        offset,
        limit,
        attachments: listed.slice(offset, offset + limit),
      };
    },
  );
}

/** Non-markdown files under the vault, sorted by path. Skips hidden entries and does not follow symlinks. */
async function collectAttachments(
  vaultPath: string,
  subdir = '',
): Promise<Array<Omit<Attachment, 'references'>>> {
  const out: Array<Omit<Attachment, 'references'>> = [];
  const entries = await readdir(join(vaultPath, subdir), { withFileTypes: true });
  for (const entry of entries) {
    if (entry.name.startsWith('.')) continue;
    const rel = subdir ? `${subdir}/${entry.name}` : entry.name;
    if (entry.isDirectory()) {
      out.push(...(await collectAttachments(vaultPath, rel)));
    } else if (entry.isFile() && !entry.name.toLowerCase().endsWith('.md')) {
      const { size } = await stat(join(vaultPath, rel));
      const dot = entry.name.lastIndexOf('.');
      out.push({ path: rel, extension: dot > 0 ? entry.name.slice(dot + 1).toLowerCase() : '', size });
    }
  }
  return out.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
}

/** Map each attachment path to the number of distinct notes that reference it. */
function countReferences(ctx: ServerContext, paths: string[]): Map<string, number> {
  const byPath = new Map<string, string>();
  const byName = new Map<string, string[]>();
  for (const p of paths) {
    byPath.set(p.toLowerCase(), p);
    const name = posix.basename(p).toLowerCase();
    byName.set(name, [...(byName.get(name) ?? []), p]);
  }

  const counts = new Map<string, number>();
  const notes = ctx.db
    .prepare("SELECT id, content FROM nodes WHERE id NOT LIKE '\\_stub/%' ESCAPE '\\'")
    .all() as Array<{ id: string; content: string | null }>;
  for (const note of notes) {
    const hits = new Set<string>();
    for (const target of linkTargets(note.content ?? '')) {
      const hit = resolveAttachment(target, note.id, byPath, byName);
      if (hit !== undefined) hits.add(hit);
    }
    for (const hit of hits) counts.set(hit, (counts.get(hit) ?? 0) + 1);
  }
  return counts;
}

/** Targets of `[[x]]`, `![[x]]`, `[](x)` and `![](x)`, outside code, without `#heading`, `|alias` or URL schemes. */
function linkTargets(markdown: string): string[] {
  const text = markdown.replace(/```[\s\S]*?```/g, '').replace(/`[^`\n]+`/g, '');
  const targets: string[] = [];
  for (const m of text.matchAll(/\[\[([^\]]+)\]\]/g)) {
    targets.push(m[1]!.split('|')[0]!.split(/[#^]/)[0]!.trim());
  }
  for (const m of text.matchAll(/\[[^\]]*\]\(\s*(<[^>]+>|[^)\s]+)(?:\s+"[^"]*")?\s*\)/g)) {
    let target = m[1]!.replace(/^<|>$/g, '');
    if (/^[a-z][a-z0-9+.-]*:/i.test(target)) continue;
    target = target.split(/[#?]/)[0]!;
    try {
      target = decodeURI(target);
    } catch {
      // Keep the raw target when it holds a stray `%`.
    }
    targets.push(target.trim());
  }
  return targets.filter((t) => t !== '');
}

function resolveAttachment(
  target: string,
  noteId: string,
  byPath: Map<string, string>,
  byName: Map<string, string[]>,
): string | undefined {
  const t = target.replace(/^\/+/, '').toLowerCase();
  const exact = byPath.get(posix.normalize(t));
  if (exact) return exact;

  const noteDir = posix.dirname(noteId).toLowerCase();
  const relative = byPath.get(posix.normalize(posix.join(noteDir, t)));
  if (relative) return relative;

  const candidates = (byName.get(posix.basename(t)) ?? []).filter(
    (p) => !t.includes('/') || p.toLowerCase().endsWith(`/${t}`),
  );
  // Several files share the name: Obsidian prefers the one beside the note.
  return candidates.find((p) => posix.dirname(p).toLowerCase() === noteDir) ?? candidates[0];
}
