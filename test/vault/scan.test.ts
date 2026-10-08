import { afterEach, describe, expect, it } from 'vitest';
import { mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import {
  compileUserRegex,
  findHeadings,
  findWikiLinks,
  maskCode,
  normalizeFolder,
  splitFrontmatter,
  vaultFilePath,
  writeNoteFile,
} from '../../src/vault/scan.js';

describe('scan helpers', () => {
  it('normalizeFolder strips slashes and backslashes', () => {
    expect(normalizeFolder('/a\\b/')).toBe('a/b');
    expect(normalizeFolder('')).toBe('');
  });

  it('splitFrontmatter handles none, empty, CRLF and a fence at end of file', () => {
    expect(splitFrontmatter('body')).toEqual({ frontmatter: '', body: 'body' });
    expect(splitFrontmatter('---\n---\nx')).toEqual({ frontmatter: '---\n---\n', body: 'x' });
    expect(splitFrontmatter('---\r\na: 1\r\n---\r\nx').body).toBe('x');
    expect(splitFrontmatter('---\na: 1\n---')).toEqual({ frontmatter: '---\na: 1\n---', body: '' });
  });

  it('maskCode keeps length and newlines, blanks fences and inline spans', () => {
    const text = 'a `b` c\n```js\n[[x]]\n```\n~~~\nopen fence\n';
    const masked = maskCode(text);
    expect(masked).toHaveLength(text.length);
    expect(masked.split('\n')).toHaveLength(text.split('\n').length);
    expect(masked).not.toContain('b');
    expect(masked).not.toContain('[[x]]');
    expect(masked).not.toContain('open fence');
    expect(maskCode('x `y` z', true)).toBe('x `y` z');
    expect(maskCode('a ``b ` c`` d')).toBe('a           d');
  });

  it('findHeadings reads ATX headings with closing hashes, outside code', () => {
    const hs = findHeadings('# One\n#nope\n```\n# In code\n```\n### Three ###\n#  \n');
    expect(hs.map((h) => [h.level, h.text, h.line])).toEqual([
      [1, 'One', 1],
      [3, 'Three', 6],
    ]);
  });

  it('findWikiLinks splits target, subpath and alias', () => {
    const [a, b, c] = findWikiLinks('[[N#H|al]] ![[M^b]] [[#S]]');
    expect(a).toMatchObject({ embed: false, target: 'N', subpath: 'H', alias: 'al' });
    expect(b).toMatchObject({ embed: true, target: 'M', subpath: '^b', alias: null });
    expect(c).toMatchObject({ target: '', subpath: 'S' });
  });

  it('compileUserRegex guards length, nested quantifiers and syntax', () => {
    expect(() => compileUserRegex('a'.repeat(501), 'g')).toThrow(/longer than 500/);
    expect(() => compileUserRegex('(a+)+', 'g')).toThrow(/nested quantifier/);
    expect(() => compileUserRegex('(a|b*){2,}', 'g')).toThrow(/nested quantifier/);
    expect(() => compileUserRegex('(', 'g')).toThrow(/Invalid regex/);
    expect(compileUserRegex('(a+)', 'g').test('aa')).toBe(true);
    expect(compileUserRegex('(a+)+', 'g', true).test('x(a+)+y')).toBe(true);
  });
});

describe('vault path guard', () => {
  let vault: string;
  afterEach(async () => {
    await rm(vault, { recursive: true, force: true });
  });

  it('allows files inside, refuses .., absolute and symlink escapes', async () => {
    const root = await mkdtemp(join(tmpdir(), 'kg-scan-'));
    vault = root;
    await writeFile(join(root, 'in.md'), 'x');
    await writeFile(join(tmpdir(), 'kg-scan-outside.md'), 'secret');
    await symlink(join(tmpdir(), 'kg-scan-outside.md'), join(root, 'link.md'));

    await expect(vaultFilePath(root, 'in.md')).resolves.toMatch(/in\.md$/);
    await expect(vaultFilePath(root, '../x.md')).rejects.toThrow(/escapes the vault/);
    await expect(vaultFilePath(root, '/etc/passwd')).rejects.toThrow(/escapes the vault/);
    await expect(vaultFilePath(root, 'link.md')).rejects.toThrow(/escapes the vault/);
    await expect(writeNoteFile(root, 'link.md', 'pwned')).rejects.toThrow(/escapes the vault/);
    expect(await readFile(join(tmpdir(), 'kg-scan-outside.md'), 'utf-8')).toBe('secret');

    await writeNoteFile(root, 'in.md', 'new');
    expect(await readFile(join(root, 'in.md'), 'utf-8')).toBe('new');
    await rm(join(tmpdir(), 'kg-scan-outside.md'));
  });
});
