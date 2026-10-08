/**
 * Turn a client-supplied path into a vault-relative path and an absolute
 * path, refusing anything that would land outside the vault.
 *
 * Three escapes are refused: an absolute path, a `..` segment that climbs
 * above the root, and a symlink whose target lies outside the vault. The
 * symlink check resolves the deepest part of the path that exists, so it
 * also covers a path that does not exist yet (a folder or file to create).
 */

import { lstatSync, realpathSync } from 'fs';
import { dirname, isAbsolute, join, posix, sep } from 'path';

export interface VaultPath {
  /** Vault-relative, `/`-separated, no leading or trailing slash. `''` is the root. */
  rel: string;
  abs: string;
}

export function resolveVaultPath(vaultPath: string, input: string): VaultPath {
  const cleaned = input.trim().replace(/\\/g, '/');
  if (isAbsolute(cleaned) || cleaned.startsWith('/') || /^[A-Za-z]:/.test(cleaned)) {
    throw new Error(`Path "${input}" is outside the vault: use a vault-relative path`);
  }
  let rel = posix.normalize(cleaned === '' ? '.' : cleaned).replace(/\/+$/, '');
  if (rel === '.') rel = '';
  if (rel === '..' || rel.startsWith('../')) {
    throw new Error(`Path "${input}" is outside the vault`);
  }

  const abs = rel === '' ? vaultPath : join(vaultPath, rel);
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
    throw new Error(`Path "${input}" is outside the vault`);
  }
  if (real !== root && !real.startsWith(root + sep)) {
    throw new Error(`Path "${input}" is outside the vault`);
  }
  return { rel, abs };
}

function entryExists(abs: string): boolean {
  try {
    lstatSync(abs);
    return true;
  } catch {
    return false;
  }
}
