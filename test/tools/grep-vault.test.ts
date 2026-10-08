import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { mkdtemp, rm, symlink } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import {
  GREP_TIME_BUDGET_MS,
  compilePattern,
  registerGrepVaultTool,
} from '../../src/tools/grep-vault.js';
import {
  acceptsArgs,
  buildCtx,
  errorText,
  makeSchemaServer,
  unwrap,
  writeVault,
  type SchemaTool,
} from '../helpers/search-tools.js';
import type { DatabaseHandle } from '../../src/store/db.js';

describe('compilePattern', () => {
  it('escapes metacharacters in a literal query', () => {
    const re = compilePattern('a.b (c)', false, true);
    expect(re.test('xa.b (c)y')).toBe(true);
    expect(re.test('aXb (c)')).toBe(false);
  });

  it('is case-insensitive unless caseSensitive', () => {
    expect(compilePattern('Foo', false, false).test('foo')).toBe(true);
    expect(compilePattern('Foo', false, true).test('foo')).toBe(false);
  });

  it('refuses long, nested-quantifier and invalid regexes', () => {
    expect(() => compilePattern('a'.repeat(501), true, false)).toThrow(/too long/);
    expect(() => compilePattern('(a+)+$', true, false)).toThrow(/quantified group/);
    expect(() => compilePattern('(\\w*x)*', true, false)).toThrow(/quantified group/);
    expect(() => compilePattern('(a{1,})+', true, false)).toThrow(/quantified group/);
    expect(() => compilePattern('(', true, false)).toThrow(/Invalid regex/);
  });

  it('accepts ordinary groups and escaped parentheses', () => {
    expect(compilePattern('(foo|bar)+', true, false).test('barfoo')).toBe(true);
    expect(compilePattern('\\(a+\\)+', true, false).test('(aa))')).toBe(true);
    expect(compilePattern('a'.repeat(500), true, false)).toBeInstanceOf(RegExp);
  });
});

describe('tools/grep_vault', () => {
  let vault: string;
  let tool: SchemaTool;

  beforeEach(async () => {
    vault = await mkdtemp(join(tmpdir(), 'kg-grep-'));
    await writeVault(vault, {
      'Alpha.md': ['---', 'status: draft', '---', '# Alpha', 'first line', 'the Widget is here', 'last line'].join('\n'),
      'Projects/Beta.md': ['widget one', 'widget two', 'widget three'].join('\r\n'),
      'Projects/Deep/Gamma.md': 'no match here\n',
      '.obsidian/workspace.md': 'widget in config\n',
      'attachments/hidden.md': 'widget in attachments\n',
      'notes.txt': 'widget in a non-note\n',
    });
    const { server, registered } = makeSchemaServer();
    registerGrepVaultTool(server, buildCtx({} as DatabaseHandle, vault));
    tool = registered[0]!;
  });

  afterEach(async () => {
    vi.restoreAllMocks();
    await rm(vault, { recursive: true, force: true });
  });

  it('finds literal matches with real 1-based line numbers and one line of context', async () => {
    const out = unwrap(await tool.cb({ query: 'widget' }));
    expect(out.totalFiles).toBe(3);
    expect(out.scannedFiles).toBe(3);
    expect(out.truncated).toBeUndefined();
    expect(out.files.map((f: { path: string }) => f.path)).toEqual(['Alpha.md', 'Projects/Beta.md']);
    expect(out.files[0].matches).toEqual([
      { line: 6, text: 'the Widget is here', before: ['first line'], after: ['last line'] },
    ]);
    // CRLF files split cleanly, and context stops at the file edges.
    expect(out.files[1].matches[0]).toEqual({ line: 1, text: 'widget one', before: [], after: ['widget two'] });
    expect(out.files[1].matches[2]).toEqual({ line: 3, text: 'widget three', before: ['widget two'], after: [] });
  });

  it('searches frontmatter too', async () => {
    const out = unwrap(await tool.cb({ query: 'status: draft', contextLines: 0 }));
    expect(out.files[0]).toEqual({ path: 'Alpha.md', matches: [{ line: 2, text: 'status: draft' }] });
  });

  it('caseSensitive narrows the match', async () => {
    const out = unwrap(await tool.cb({ query: 'Widget', caseSensitive: true }));
    expect(out.files.map((f: { path: string }) => f.path)).toEqual(['Alpha.md']);
  });

  it('regex mode matches a pattern; a literal query treats metacharacters as text', async () => {
    const re = unwrap(await tool.cb({ query: '^widget (one|three)$', regex: true, contextLines: 0 }));
    expect(re.files[0].matches.map((m: { line: number }) => m.line)).toEqual([1, 3]);
    const lit = unwrap(await tool.cb({ query: 'widget (one|three)' }));
    expect(lit.files).toEqual([]);
  });

  it('contextLines widens the window', async () => {
    const out = unwrap(await tool.cb({ query: 'widget is', contextLines: 2 }));
    expect(out.files[0].matches[0].before).toEqual(['# Alpha', 'first line']);
  });

  it('maxMatchesPerFile caps lines and flags the rest; exactly at the cap is not flagged', async () => {
    const capped = unwrap(await tool.cb({ query: 'widget', folder: 'Projects', maxMatchesPerFile: 2 }));
    expect(capped.files[0].matches).toHaveLength(2);
    expect(capped.files[0].moreMatches).toBe(true);
    const exact = unwrap(await tool.cb({ query: 'widget', folder: 'Projects', maxMatchesPerFile: 3 }));
    expect(exact.files[0].matches).toHaveLength(3);
    expect(exact.files[0].moreMatches).toBeUndefined();
  });

  it('limit stops at that many matching files and reports truncation', async () => {
    const out = unwrap(await tool.cb({ query: 'widget', limit: 1 }));
    expect(out.files).toHaveLength(1);
    expect(out.truncated).toBe(true);
    expect(out.stoppedBy).toBe('limit');
    expect(out.scannedFiles).toBeLessThan(out.totalFiles);
  });

  it('folder restricts the scan, recursively', async () => {
    const out = unwrap(await tool.cb({ query: 'match', folder: 'Projects' }));
    expect(out.totalFiles).toBe(2);
    expect(out.files.map((f: { path: string }) => f.path)).toEqual(['Projects/Deep/Gamma.md']);
  });

  it('clips very long lines around the match', async () => {
    await writeVault(vault, { 'Long.md': 'x'.repeat(2000) + 'NEEDLE' + 'y'.repeat(2000) + '\nshort\n' });
    const out = unwrap(await tool.cb({ query: 'needle' }));
    const m = out.files.find((f: { path: string }) => f.path === 'Long.md').matches[0];
    expect(m.text).toContain('NEEDLE');
    expect(m.text.length).toBeLessThanOrEqual(402);
    expect(m.text.startsWith('…') && m.text.endsWith('…')).toBe(true);
  });

  it('stops at the time budget and returns what it has', async () => {
    let now = 1_000_000;
    vi.spyOn(Date, 'now').mockImplementation(() => {
      now += GREP_TIME_BUDGET_MS / 2 + 1;
      return now;
    });
    const out = unwrap(await tool.cb({ query: 'widget' }));
    expect(out.truncated).toBe(true);
    expect(out.stoppedBy).toBe('time');
    expect(out.scannedFiles).toBeLessThan(out.totalFiles);
  });

  it('keeps the matches of a file cut off mid-scan by the time budget', async () => {
    const lines = Array.from({ length: 600 }, (_, i) => (i === 3 ? 'needle' : 'filler'));
    await writeVault(vault, { 'AAA.md': lines.join('\n') });
    let calls = 0;
    vi.spyOn(Date, 'now').mockImplementation(() => (++calls > 3 ? 1e12 : 0));
    const out = unwrap(await tool.cb({ query: 'needle' }));
    expect(out.stoppedBy).toBe('time');
    expect(out.files).toEqual([{ path: 'AAA.md', matches: [{ line: 4, text: 'needle', before: ['filler'], after: ['filler'] }] }]);
    expect(out.scannedFiles).toBe(0);
  });

  it('rejects unsafe and invalid regexes as tool errors', async () => {
    expect(errorText(await tool.cb({ query: '(a+)+', regex: true }))).toMatch(/quantified group/);
    expect(errorText(await tool.cb({ query: '[', regex: true }))).toMatch(/Invalid regex/);
  });

  it('refuses folders outside the vault, including through a symlink', async () => {
    const outside = await mkdtemp(join(tmpdir(), 'kg-grep-outside-'));
    try {
      await writeVault(outside, { 'secret.md': 'widget secret\n' });
      await symlink(outside, join(vault, 'Escape'));
      expect(errorText(await tool.cb({ query: 'widget', folder: 'Escape' }))).toMatch(/inside the vault/);
      expect(errorText(await tool.cb({ query: 'widget', folder: '../x' }))).toMatch(/inside the vault/);
      expect(errorText(await tool.cb({ query: 'widget', folder: '/etc' }))).toMatch(/vault-relative/);
    } finally {
      await rm(outside, { recursive: true, force: true });
    }
  });

  it('an unknown folder or a file given as folder is an error', async () => {
    expect(errorText(await tool.cb({ query: 'w', folder: 'Nope' }))).toMatch(/Folder not found/);
    expect(errorText(await tool.cb({ query: 'w', folder: 'Alpha.md' }))).toMatch(/Folder not found/);
  });

  it('schema bounds the numeric options and requires a query', () => {
    expect(acceptsArgs(tool, { query: '' })).toBe(false);
    expect(acceptsArgs(tool, { query: 'a', contextLines: 11 })).toBe(false);
    expect(acceptsArgs(tool, { query: 'a', limit: 0 })).toBe(false);
    expect(acceptsArgs(tool, { query: 'a', maxMatchesPerFile: 101 })).toBe(false);
    expect(acceptsArgs(tool, { query: 'a', regex: true, caseSensitive: true, contextLines: 0 })).toBe(true);
  });
});
