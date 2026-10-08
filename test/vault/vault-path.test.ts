import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdtemp, mkdir, writeFile, readFile, rm, symlink, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import {
  inFolder,
  normalizeFolder,
  readNoteFile,
  resolveFolder,
  resolveVaultPath,
  toVaultRelative,
  writeFileAtomic,
} from '../../src/vault/vault-path.js';

let vault: string;
let outside: string;

beforeEach(async () => {
  vault = await mkdtemp(join(tmpdir(), 'kg-vault-path-'));
  outside = await mkdtemp(join(tmpdir(), 'kg-vault-path-out-'));
  await mkdir(join(vault, 'A'));
  await writeFile(join(vault, 'A', 'Note.md'), 'x');
  await writeFile(join(outside, 'Secret.md'), 'secret');
});

afterEach(async () => {
  await rm(vault, { recursive: true, force: true });
  await rm(outside, { recursive: true, force: true });
});

describe('resolveVaultPath', () => {
  it('accepts paths inside the vault, existing or not', () => {
    expect(resolveVaultPath(vault, 'A/Note.md')).toEqual({ rel: 'A/Note.md', abs: join(vault, 'A', 'Note.md') });
    expect(resolveVaultPath(vault, 'A/../A/New.md')).toEqual({ rel: 'A/New.md', abs: join(vault, 'A', 'New.md') });
    expect(resolveVaultPath(vault, 'New/Deep/x.md').rel).toBe('New/Deep/x.md');
    expect(resolveVaultPath(vault, '')).toEqual({ rel: '', abs: vault });
    expect(resolveVaultPath(vault, '.')).toEqual({ rel: '', abs: vault });
  });

  it('uses / separators and drops trailing slashes', () => {
    expect(resolveVaultPath(vault, 'A\\Note.md').rel).toBe('A/Note.md');
    expect(resolveVaultPath(vault, 'A/').rel).toBe('A');
  });

  it('refuses absolute paths', () => {
    for (const p of [join(vault, 'A', 'Note.md'), '/etc/passwd', '\\etc', 'C:/temp', 'c:\\temp']) {
      expect(() => resolveVaultPath(vault, p), p).toThrow(/outside the vault: use a vault-relative path/);
    }
  });

  it('refuses .. escapes', () => {
    for (const p of ['..', '../x.md', 'A/../../x.md', '..\\x.md']) {
      expect(() => resolveVaultPath(vault, p), p).toThrow(`Path "${p}" is outside the vault`);
    }
  });

  it('refuses NUL bytes', () => {
    expect(() => resolveVaultPath(vault, 'A/x\0.md')).toThrow(/contains a NUL byte/);
  });

  it('refuses a symlinked file or folder that points outside', async () => {
    await symlink(join(outside, 'Secret.md'), join(vault, 'Linked.md'));
    await symlink(outside, join(vault, 'Out'));
    expect(() => resolveVaultPath(vault, 'Linked.md')).toThrow(/outside the vault through a symlink/);
    expect(() => resolveVaultPath(vault, 'Out')).toThrow(/outside the vault through a symlink/);
    expect(() => resolveVaultPath(vault, 'Out/Secret.md')).toThrow(/outside the vault through a symlink/);
  });

  it('refuses a non-existent nested path under a symlinked folder', async () => {
    await symlink(outside, join(vault, 'Out'));
    expect(() => resolveVaultPath(vault, 'Out/new/deeper/x.md')).toThrow(/through a symlink/);
  });

  it('refuses a dangling symlink and paths under it', async () => {
    await symlink(join(outside, 'missing'), join(vault, 'dangling'));
    expect(() => resolveVaultPath(vault, 'dangling')).toThrow(/through a symlink/);
    expect(() => resolveVaultPath(vault, 'dangling/x.md')).toThrow(/through a symlink/);
  });

  it('accepts a symlink whose target stays inside the vault', async () => {
    await symlink(join(vault, 'A'), join(vault, 'Alias'));
    expect(resolveVaultPath(vault, 'Alias/Note.md').abs).toBe(join(vault, 'Alias', 'Note.md'));
    expect(resolveVaultPath(vault, 'Alias/New.md').rel).toBe('Alias/New.md');
  });
});

describe('toVaultRelative', () => {
  it('returns a /-separated path relative to the vault', () => {
    expect(toVaultRelative(vault, join(vault, 'A', 'Note.md'))).toBe('A/Note.md');
    expect(toVaultRelative(vault, vault)).toBe('');
  });
});

describe('normalizeFolder and inFolder', () => {
  it('normalises slashes, backslashes and dots, rooted at the vault', () => {
    expect(normalizeFolder('/a\\b/')).toBe('a/b');
    expect(normalizeFolder('./a/./b')).toBe('a/b');
    expect(normalizeFolder(' a/b ')).toBe('a/b');
  });

  it('maps an absent folder, empty string and / to the root', () => {
    expect(normalizeFolder(undefined)).toBe('');
    expect(normalizeFolder('')).toBe('');
    expect(normalizeFolder('/')).toBe('');
    expect(normalizeFolder('.')).toBe('');
  });

  it('refuses .., drive letters and NUL bytes', () => {
    expect(() => normalizeFolder('..')).toThrow(/outside the vault/);
    expect(() => normalizeFolder('a/../../b')).toThrow(/outside the vault/);
    expect(() => normalizeFolder('C:/x')).toThrow(/vault-relative/);
    expect(() => normalizeFolder('a\0')).toThrow(/NUL byte/);
  });

  it('inFolder matches ids under the folder only', () => {
    expect(inFolder('a/b/c.md', '')).toBe(true);
    expect(inFolder('a/b/c.md', 'a')).toBe(true);
    expect(inFolder('a/b/c.md', 'a/b')).toBe(true);
    expect(inFolder('ab/c.md', 'a')).toBe(false);
    expect(inFolder('a.md', 'a')).toBe(false);
  });
});

describe('resolveFolder', () => {
  it('returns the folder when it is a directory', async () => {
    expect(await resolveFolder(vault, '/A/')).toEqual({ rel: 'A', abs: join(vault, 'A') });
    expect(await resolveFolder(vault, undefined)).toEqual({ rel: '', abs: vault });
  });

  it('errors on a missing folder or a file', async () => {
    await expect(resolveFolder(vault, 'Nope')).rejects.toThrow('Folder not found: "Nope"');
    await expect(resolveFolder(vault, 'A/Note.md')).rejects.toThrow('Folder not found: "A/Note.md"');
  });

  it('refuses a folder outside the vault', async () => {
    await symlink(outside, join(vault, 'Out'));
    await expect(resolveFolder(vault, 'Out')).rejects.toThrow(/through a symlink/);
    await expect(resolveFolder(vault, '../x')).rejects.toThrow(/outside the vault/);
  });
});

describe('readNoteFile', () => {
  it('reads a file inside the vault', async () => {
    expect(await readNoteFile(vault, 'A/Note.md')).toEqual({
      rel: 'A/Note.md',
      abs: join(vault, 'A', 'Note.md'),
      content: 'x',
    });
  });

  it('errors on a missing file and an escape', async () => {
    await expect(readNoteFile(vault, 'A/Gone.md')).rejects.toThrow(/Could not read "A\/Gone.md": ENOENT/);
    await symlink(join(outside, 'Secret.md'), join(vault, 'Linked.md'));
    await expect(readNoteFile(vault, 'Linked.md')).rejects.toThrow(/outside the vault/);
  });
});

describe('writeFileAtomic', () => {
  it('replaces a file with a string and leaves no temp file', async () => {
    await writeFileAtomic(join(vault, 'A', 'Note.md'), 'new ü');
    expect(await readFile(join(vault, 'A', 'Note.md'), 'utf-8')).toBe('new ü');
    expect(await readdir(join(vault, 'A'))).toEqual(['Note.md']);
  });

  it('writes a Buffer byte for byte', async () => {
    const bytes = Buffer.from([0, 255, 1, 128, 10]);
    await writeFileAtomic(join(vault, 'A', 'blob.bin'), bytes);
    expect(Buffer.compare(await readFile(join(vault, 'A', 'blob.bin')), bytes)).toBe(0);
    expect((await readdir(join(vault, 'A'))).sort()).toEqual(['Note.md', 'blob.bin']);
  });

  it('lets concurrent writers finish without a shared temp file', async () => {
    const target = join(vault, 'A', 'Note.md');
    await Promise.all(['one', 'two', 'three'].map((c) => writeFileAtomic(target, c)));
    expect(['one', 'two', 'three']).toContain(await readFile(target, 'utf-8'));
    expect(await readdir(join(vault, 'A'))).toEqual(['Note.md']);
  });

  it('removes the temp file when the rename fails', async () => {
    await mkdir(join(vault, 'A', 'Dir'));
    await writeFile(join(vault, 'A', 'Dir', 'child'), '');
    await expect(writeFileAtomic(join(vault, 'A', 'Dir'), 'x')).rejects.toThrow();
    expect((await readdir(join(vault, 'A'))).sort()).toEqual(['Dir', 'Note.md']);
  });
});
