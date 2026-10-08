import { z } from 'zod';
import { lstat, readdir, rm, rmdir } from 'node:fs/promises';
import { join } from 'node:path';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import { runBackgroundReindex } from './background-reindex.js';
import type { ServerContext } from '../context.js';
import { resolveVaultPath } from '../vault/vault-path.js';
import { deleteNote } from '../vault/mover.js';

/** How many paths a dry run lists before it truncates. */
const DRY_RUN_SAMPLE = 50;

/**
 * `delete_folder` — remove a folder from disk. Every note under it goes
 * through `deleteNote` first, so its node, edges, embedding and orphaned
 * stubs leave the index in the same call. Indexed notes under the folder
 * that are already gone from disk are purged too.
 */
export function registerDeleteFolderTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'delete_folder',
    'Permanently delete a folder. Refuses a non-empty folder unless `recursive: true`; notes inside also leave the index. Requires `confirm: true`; `dryRun` lists what would go.',
    {
      path: z.string().describe('Vault-relative folder path.'),
      confirm: z.literal(true).describe('Must literally be `true` to execute. Guards against accidental deletion.'),
      recursive: z.boolean().optional().describe('Delete the folder with everything in it. Default false: refuse a non-empty folder.'),
      dryRun: z.boolean().optional().describe('If true, report what would be deleted without removing anything.'),
    },
    async (args) => {
      const { rel, abs } = resolveVaultPath(ctx.config.vaultPath, args.path);
      if (rel === '') throw new Error('Refusing to delete the vault root');
      if (rel === '.obsidian' || rel.startsWith('.obsidian/')) {
        throw new Error('Refusing to delete the .obsidian config folder');
      }
      const st = await lstat(abs).catch(() => {
        throw new Error(`Folder not found: "${rel}"`);
      });
      if (st.isSymbolicLink()) throw new Error(`${rel} is a symlink, not a folder`);
      if (!st.isDirectory()) throw new Error(`${rel} is a file, not a folder: use delete_note`);

      const { files, folders } = await walk(abs, rel);
      const notes = [
        ...new Set([...files.filter((f) => f.toLowerCase().endsWith('.md')), ...indexedNotesUnder(ctx, rel)]),
      ];
      const empty = files.length === 0 && folders.length === 0;
      const refusal = !empty && args.recursive !== true
        ? `Folder ${rel} is not empty (${files.length} files, ${folders.length} folders). Pass recursive: true to delete it with its contents.`
        : undefined;

      if (args.dryRun === true) {
        const paths = [...folders, ...files].sort();
        return {
          dryRun: true,
          path: rel,
          files: files.length,
          folders: folders.length,
          notes: notes.length,
          sample: paths.slice(0, DRY_RUN_SAMPLE),
          truncated: paths.length > DRY_RUN_SAMPLE,
          ...(refusal ? { refused: refusal } : {}),
        };
      }
      if (refusal) throw new Error(refusal);

      const fromIndex = { nodes: 0, edges: 0, stubsPruned: 0 };
      for (const note of notes) {
        const result = await deleteNote(ctx.config.vaultPath, note, ctx.db);
        if (result.deletedFromIndex.node) fromIndex.nodes++;
        fromIndex.edges += result.deletedFromIndex.edges;
        fromIndex.stubsPruned += result.deletedFromIndex.stubsPruned;
      }
      if (empty) await rmdir(abs);
      else await rm(abs, { recursive: true });

      // The notes left the index above; the reindex re-resolves the links
      // that pointed at them, which become stubs.
      runBackgroundReindex(ctx);

      return {
        path: rel,
        deleted: { files: files.length, folders: folders.length, notes: notes.length },
        deletedFromIndex: fromIndex,
      };
    },
  );
}

/** Every file and folder under `abs`, as vault-relative paths. Symlinks count as files and are not followed. */
async function walk(abs: string, rel: string): Promise<{ files: string[]; folders: string[] }> {
  const files: string[] = [];
  const folders: string[] = [];
  for (const entry of await readdir(abs, { withFileTypes: true })) {
    const childRel = `${rel}/${entry.name}`;
    if (entry.isDirectory()) {
      folders.push(childRel);
      const sub = await walk(join(abs, entry.name), childRel);
      files.push(...sub.files);
      folders.push(...sub.folders);
    } else {
      files.push(childRel);
    }
  }
  return { files, folders };
}

function indexedNotesUnder(ctx: ServerContext, rel: string): string[] {
  const prefix = `${rel}/`;
  return (
    ctx.db
      .prepare('SELECT id FROM nodes WHERE substr(id, 1, ?) = ?')
      .all(prefix.length, prefix) as Array<{ id: string }>
  ).map((r) => r.id);
}
