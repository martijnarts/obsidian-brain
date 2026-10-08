import { existsSync, realpathSync } from 'fs';
import { dirname, isAbsolute, relative, resolve, sep } from 'path';

/**
 * Turn a client-supplied vault-relative path into an absolute path, and
 * refuse anything that lands outside the vault: absolute paths, `..`
 * segments that climb out, and symlinks that point elsewhere. The target
 * itself need not exist; its nearest existing ancestor is what gets
 * resolved through symlinks.
 */
export function resolveInVault(vaultPath: string, relPath: string): string {
  if (relPath.includes('\0')) {
    throw new Error(`Invalid path: ${JSON.stringify(relPath)}`);
  }
  if (isAbsolute(relPath) || /^[a-zA-Z]:[\\/]/.test(relPath)) {
    throw new Error(`Path must be vault-relative, got absolute path: ${relPath}`);
  }
  const root = resolve(vaultPath);
  const abs = resolve(root, relPath);
  if (!isInside(root, abs)) {
    throw new Error(`Path escapes the vault: ${relPath}`);
  }

  let existing = abs;
  while (!existsSync(existing) && existing !== root) {
    existing = dirname(existing);
  }
  const realRoot = realpathSync(root);
  const realExisting = realpathSync(existing);
  if (!isInside(realRoot, realExisting)) {
    throw new Error(`Path escapes the vault through a symlink: ${relPath}`);
  }
  return abs;
}

/** Vault-relative form of `abs`, always with `/` separators. */
export function toVaultRelative(vaultPath: string, abs: string): string {
  return relative(resolve(vaultPath), abs).split(sep).join('/');
}

function isInside(root: string, candidate: string): boolean {
  if (candidate === root) return true;
  const rel = relative(root, candidate);
  return rel !== '..' && !rel.startsWith(`..${sep}`) && !isAbsolute(rel);
}
