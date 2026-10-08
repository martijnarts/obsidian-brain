import { promises as fs } from 'node:fs';
import type { ServerContext } from '../context.js';
import { resolveToSinglePath } from './delete-note.js';
import { resolveInVault } from '../vault/vault-path.js';
import { errorMessage } from '../util/errors.js';

export interface NoteFile {
  /** Vault-relative path. */
  path: string;
  abs: string;
  content: string;
}

/** Resolve `name` to one note and read it from disk. */
export async function readNoteFile(name: string, ctx: ServerContext): Promise<NoteFile> {
  const path = resolveToSinglePath(name, ctx);
  const abs = await resolveInVault(ctx.config.vaultPath, path);
  try {
    return { path, abs, content: await fs.readFile(abs, 'utf-8') };
  } catch (err) {
    throw new Error(`Could not read "${path}": ${errorMessage(err)}`);
  }
}
