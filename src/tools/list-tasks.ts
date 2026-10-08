import { z } from 'zod';
import { promises as fs } from 'node:fs';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';
import { readNoteFile } from './note-file.js';
import { collectMarkdownFiles } from '../vault/parser.js';
import { resolveInVault } from '../vault/vault-path.js';
import { scanTasks, type Task } from '../vault/tasks.js';

/**
 * `list_tasks` — markdown tasks read from the files on disk, so a task
 * ticked a moment ago shows its new state before the index catches up.
 */
export function registerListTasksTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'list_tasks',
    'List markdown tasks (`- [ ] text`) in the vault, one folder, or one note, ordered by path and line. Each task has its 1-based `line`, raw `status` character, `state` (open/done/cancelled), `text`, nesting `indent`, `parentLine` when nested under another task, and `due` from `📅 YYYY-MM-DD`. Use `line` with `set_task_status` or `ensure_block_id`.',
    {
      name: z.string().optional().describe('Path or fuzzy match of one note. Omit to scan the vault.'),
      folder: z.string().optional().describe('Vault-relative folder to scan recursively.'),
      status: z.enum(['open', 'done', 'all']).optional().describe(
        'Default `open`: every status but `x`, `X` and `-`. `done`: `x`/`X`. `all` adds cancelled (`-`).',
      ),
      limit: z.number().int().min(1).max(1000).optional().describe('Max tasks to return. Default 200, max 1000.'),
      offset: z.number().int().nonnegative().optional().describe('Tasks to skip, for paging. Default 0.'),
    },
    async (args) => {
      if (args.name !== undefined && args.folder !== undefined) {
        throw new Error('Pass `name` or `folder`, not both.');
      }
      const wanted = args.status ?? 'open';
      const limit = args.limit ?? 200;
      const offset = args.offset ?? 0;

      const notes: Array<{ path: string; content: string }> = [];
      if (args.name !== undefined) {
        notes.push(await readNoteFile(args.name, ctx));
      } else {
        for (const path of await notesUnder(ctx.config.vaultPath, args.folder ?? '')) {
          // A symlink out of the vault, or a file deleted mid-scan, is skipped.
          const content = await resolveInVault(ctx.config.vaultPath, path)
            .then((abs) => fs.readFile(abs, 'utf-8'))
            .catch(() => null);
          if (content !== null) notes.push({ path, content });
        }
      }

      const matched: Array<Task & { path: string }> = [];
      for (const { path, content } of notes) {
        for (const task of scanTasks(content)) {
          if (wanted !== 'all' && task.state !== wanted) continue;
          matched.push({ path, ...task });
        }
      }

      const tasks = matched.slice(offset, offset + limit);
      return {
        total: matched.length,
        offset,
        truncated: offset + tasks.length < matched.length,
        tasks,
      };
    },
  );
}

async function notesUnder(vaultPath: string, folder: string): Promise<string[]> {
  let rel = folder.replace(/^\.?\/+/, '').replace(/\/+$/, '');
  if (rel === '.') rel = '';
  if (rel.split('/').some((seg) => seg.startsWith('.') && seg !== '.' && seg !== '..')) {
    throw new Error(`Hidden folders are not scanned: "${folder}"`);
  }
  const abs = await resolveInVault(vaultPath, rel);
  const stat = await fs.stat(abs).catch(() => null);
  if (!stat?.isDirectory()) throw new Error(`Folder not found: "${folder}"`);
  const paths = await collectMarkdownFiles(vaultPath, rel);
  return paths.sort();
}
