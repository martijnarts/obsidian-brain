/**
 * Turn a client-supplied path into a vault-relative path and an absolute
 * path, refusing anything that would land outside the vault.
 *
 * Four escapes are refused: a NUL byte, an absolute path, a `..` segment
 * that climbs above the root, and a symlink whose target lies outside the
 * vault. The symlink check resolves the deepest part of the path that
 * exists, so it also covers a path that does not exist yet (a folder or
 * file to create).
 */

import { randomBytes } from 'crypto';
import { lstatSync, promises as fs, realpathSync } from 'fs';
import { dirname, isAbsolute, posix, relative, resolve, sep } from 'path';
import { errorMessage } from '../util/errors.js';

export interface VaultPath {
  /** Vault-relative, `/`-separated, no leading or trailing slash. `''` is the root. */
  rel: string;
  abs: string;
}

export function resolveVaultPath(vaultPath: string, input: string): VaultPath {
  const rel = lexicalRel(input);
  const abs = rel === '' ? resolve(vaultPath) : resolve(vaultPath, rel);
  const root = realpathSync(vaultPath);
  let existing = abs;
  while (!entryExists(existing) && dirname(existing) !== existing) {
    existing = dirname(existing);
  }
  // A dangling symlink exists but has no real path; a write through it would
  // create its target, which may lie anywhere.
  let real: string;
  try {
    real = realpathSync(existing);
  } catch {
    throw new Error(`Path "${input}" is outside the vault through a symlink`);
  }
  if (real !== root && !real.startsWith(root + sep)) {
    throw new Error(`Path "${input}" is outside the vault through a symlink`);
  }
  return { rel, abs };
}

/** The checks that need no disk: NUL, absolute paths and `..` escapes. */
function lexicalRel(input: string): string {
  if (input.includes('\0')) {
    throw new Error(`Path ${JSON.stringify(input)} contains a NUL byte`);
  }
  const cleaned = input.trim().replace(/\\/g, '/');
  if (isAbsolute(cleaned) || cleaned.startsWith('/') || /^[A-Za-z]:/.test(cleaned)) {
    throw new Error(`Path "${input}" is outside the vault: use a vault-relative path`);
  }
  const rel = posix.normalize(cleaned === '' ? '.' : cleaned).replace(/\/+$/, '');
  if (rel === '..' || rel.startsWith('../')) {
    throw new Error(`Path "${input}" is outside the vault`);
  }
  return rel === '.' ? '' : rel;
}

function entryExists(abs: string): boolean {
  try {
    lstatSync(abs);
    return true;
  } catch {
    return false;
  }
}

/** Vault-relative form of `abs`, always with `/` separators. */
export function toVaultRelative(vaultPath: string, abs: string): string {
  return relative(resolve(vaultPath), abs).split(sep).join('/');
}

/**
 * Normalise a client-supplied folder scope to `a/b` form, with the same
 * lexical checks as `resolveVaultPath`. A folder scope is always rooted at
 * the vault, so leading slashes are dropped: `/a/` is `a`, and an absent
 * folder, `''` or `/` is the root (`''`).
 */
export function normalizeFolder(folder: string | undefined): string {
  if (folder === undefined) return '';
  return lexicalRel(folder.trim().replace(/\\/g, '/').replace(/^\/+/, ''));
}

/** True when the vault-relative `id` lies under `folder` (`''` is the root). */
export function inFolder(id: string, folder: string): boolean {
  return folder === '' || id.startsWith(`${folder}/`);
}

/**
 * Resolve a folder scope on disk: normalise it, guard it, and check that
 * it is a directory. A symlinked folder passes when its target is inside
 * the vault.
 */
export async function resolveFolder(vaultPath: string, folder: string | undefined): Promise<VaultPath> {
  const resolved = resolveVaultPath(vaultPath, normalizeFolder(folder));
  const st = await fs.stat(resolved.abs).catch(() => undefined);
  if (!st?.isDirectory()) throw new Error(`Folder not found: "${folder ?? ''}"`);
  return resolved;
}

export interface NoteFile extends VaultPath {
  content: string;
}

/** Guard a vault-relative path and read the file as UTF-8. */
export async function readNoteFile(vaultPath: string, input: string): Promise<NoteFile> {
  const { rel, abs } = resolveVaultPath(vaultPath, input);
  try {
    return { rel, abs, content: await fs.readFile(abs, 'utf-8') };
  } catch (err) {
    throw new Error(`Could not read "${rel}": ${errorMessage(err)}`);
  }
}

/**
 * Atomic replace: write a uniquely named temp file next to the target, then
 * rename it over the target, so a reader never sees half a file and two
 * writers never share a temp file.
 */
export async function writeFileAtomic(abs: string, content: string | Buffer): Promise<void> {
  const tmp = `${abs}.${randomBytes(6).toString('hex')}.tmp`;
  try {
    await fs.writeFile(tmp, content, typeof content === 'string' ? 'utf-8' : undefined);
    await fs.rename(tmp, abs);
  } catch (err) {
    await fs.rm(tmp, { force: true });
    throw err;
  }
}
