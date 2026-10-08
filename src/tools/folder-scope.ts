import { realpath, stat } from 'fs/promises';
import { isAbsolute, join, relative, sep } from 'path';

/**
 * Normalise a client-supplied vault folder to `a/b` form. Absolute paths
 * and `..` segments are refused, so a scope can never name anything
 * outside the vault. An empty folder (or `/`) means the vault root and
 * comes back as `undefined`.
 */
export function normalizeFolder(folder: string | undefined): string | undefined {
  if (folder === undefined) return undefined;
  const slashed = folder.replace(/\\/g, '/');
  if (slashed !== '/' && (isAbsolute(slashed) || /^[A-Za-z]:\//.test(slashed))) {
    throw new Error(`Folder must be vault-relative: "${folder}"`);
  }
  const parts = slashed.split('/').filter((p) => p !== '' && p !== '.');
  if (parts.includes('..')) {
    throw new Error(`Folder must stay inside the vault: "${folder}"`);
  }
  return parts.length === 0 ? undefined : parts.join('/');
}

/** True when the vault-relative `id` lies under `folder` (or no folder is set). */
export function inFolder(id: string, folder: string | undefined): boolean {
  return folder === undefined || id.startsWith(folder + '/');
}

/**
 * Throw when no indexed id lies under `folder`. Used by the index-backed
 * tools so a typo in the folder reads as an error, not as zero results.
 */
export function assertFolderIndexed(ids: Iterable<string>, folder: string | undefined): void {
  if (folder === undefined) return;
  for (const id of ids) if (inFolder(id, folder)) return;
  throw new Error(`Folder not found: "${folder}"`);
}

/**
 * Resolve `folder` on disk and check that its real path (after symlinks)
 * is still inside the vault. Returns the absolute directory to scan.
 */
export async function resolveFolderOnDisk(
  vaultPath: string,
  folder: string | undefined,
): Promise<string> {
  const root = await realpath(vaultPath);
  if (folder === undefined) return root;
  let real: string;
  try {
    real = await realpath(join(root, folder));
    if (!(await stat(real)).isDirectory()) throw new Error('not a directory');
  } catch {
    throw new Error(`Folder not found: "${folder}"`);
  }
  const rel = relative(root, real);
  if (rel === '..' || rel.startsWith('..' + sep) || isAbsolute(rel)) {
    throw new Error(`Folder must stay inside the vault: "${folder}"`);
  }
  return real;
}
