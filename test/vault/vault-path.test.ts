import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdtemp, mkdir, writeFile, readFile, rm, symlink, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { resolveInVault, writeFileAtomic } from '../../src/vault/vault-path.js';

describe('resolveInVault', () => {
  let vault: string;
  let outside: string;

  beforeEach(async () => {
    vault = await mkdtemp(join(tmpdir(), 'kg-vault-path-'));
    outside = await mkdtemp(join(tmpdir(), 'kg-vault-path-out-'));
    await mkdir(join(vault, 'A'));
    await writeFile(join(vault, 'A', 'Note.md'), 'x');
  });

  afterEach(async () => {
    await rm(vault, { recursive: true, force: true });
    await rm(outside, { recursive: true, force: true });
  });

  it('accepts paths inside the vault, existing or not', async () => {
    expect(await resolveInVault(vault, 'A/Note.md')).toBe(join(vault, 'A', 'Note.md'));
    expect(await resolveInVault(vault, 'A/../A/New.md')).toBe(join(vault, 'A', 'New.md'));
    expect(await resolveInVault(vault, '')).toBe(vault);
  });

  it('rejects absolute paths, .. escapes and symlink escapes', async () => {
    await expect(resolveInVault(vault, join(vault, 'A', 'Note.md'))).rejects.toThrow(/vault-relative/);
    await expect(resolveInVault(vault, '../x.md')).rejects.toThrow(/escapes the vault/);
    await expect(resolveInVault(vault, '..')).rejects.toThrow(/escapes the vault/);
    await symlink(outside, join(vault, 'Out'));
    await expect(resolveInVault(vault, 'Out')).rejects.toThrow(/escapes the vault/);
    await symlink(join(vault, 'A'), join(vault, 'Alias'));
    expect(await resolveInVault(vault, 'Alias/Note.md')).toBe(join(vault, 'Alias', 'Note.md'));
  });
});

describe('writeFileAtomic', () => {
  it('replaces the file and leaves no temp file', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'kg-atomic-'));
    try {
      await writeFile(join(dir, 'f.md'), 'old');
      await writeFileAtomic(join(dir, 'f.md'), 'new');
      expect(await readFile(join(dir, 'f.md'), 'utf-8')).toBe('new');
      expect(await readdir(dir)).toEqual(['f.md']);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
});
