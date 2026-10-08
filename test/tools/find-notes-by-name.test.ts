import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdtemp, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { openDb, type DatabaseHandle } from '../../src/store/db.js';
import {
  noteAliases,
  registerFindNotesByNameTool,
  scoreName,
} from '../../src/tools/find-notes-by-name.js';
import {
  acceptsArgs,
  buildCtx,
  errorText,
  indexVault,
  makeSchemaServer,
  unwrap,
  writeVault,
  type SchemaTool,
} from '../helpers/search-tools.js';

describe('scoreName', () => {
  it('ranks exact > prefix > word-boundary substring > substring > words > subsequence', () => {
    const exact = scoreName('kelly', 'Kelly');
    const prefix = scoreName('kel', 'Kelly criterion');
    const boundary = scoreName('crit', 'Kelly criterion');
    const inner = scoreName('elly', 'Kelly criterion');
    const words = scoreName('criterion kelly', 'Kelly criterion');
    const subseq = scoreName('klcr', 'Kelly criterion');
    expect(exact).toBe(1);
    expect(prefix).toBeGreaterThan(boundary);
    expect(boundary).toBeGreaterThan(inner);
    expect(inner).toBeGreaterThan(words);
    expect(words).toBeGreaterThan(subseq);
    expect(subseq).toBeGreaterThan(0);
  });

  it('tolerates a typo through Levenshtein similarity', () => {
    expect(scoreName('kubernets', 'Kubernetes')).toBeGreaterThan(0.4);
  });

  it('returns 0 for no match and for empty input', () => {
    expect(scoreName('zzz', 'Kelly')).toBe(0);
    expect(scoreName('  ', 'Kelly')).toBe(0);
    expect(scoreName('a', '')).toBe(0);
  });
});

describe('noteAliases', () => {
  it('reads a list, a comma-separated string and the legacy `alias` key', () => {
    expect(noteAliases({ aliases: ['A', ' B ', '', 3] })).toEqual(['A', 'B']);
    expect(noteAliases({ aliases: 'One, Two' })).toEqual(['One', 'Two']);
    expect(noteAliases({ alias: 'Old' })).toEqual(['Old']);
    expect(noteAliases({})).toEqual([]);
  });
});

describe('tools/find_notes_by_name', () => {
  let vault: string;
  let db: DatabaseHandle;
  let tool: SchemaTool;

  beforeEach(async () => {
    vault = await mkdtemp(join(tmpdir(), 'kg-find-name-'));
    db = openDb(':memory:');
    await writeVault(vault, {
      'Finance/Kelly criterion.md': '# Kelly\n\nSee [[Missing note]].\n',
      'Finance/Kelly notes.md': '---\ntitle: Betting sizes\n---\nbody\n',
      'People/Bob Smith.md': '---\naliases: [Robert, Bobby Tables]\n---\nhi\n',
      'People/Robert Jones.md': 'hi\n',
      'Inbox/Robbery report.md': 'hi\n',
    });
    await indexVault(db, vault);
    const { server, registered } = makeSchemaServer();
    registerFindNotesByNameTool(server, buildCtx(db, vault));
    tool = registered[0]!;
  });

  afterEach(async () => {
    db.close();
    await rm(vault, { recursive: true, force: true });
  });

  it('ranks the exact name first and reports what matched', async () => {
    const out = unwrap(await tool.cb({ query: 'kelly criterion' }));
    expect(out.results[0]).toEqual({
      path: 'Finance/Kelly criterion.md',
      title: 'Kelly criterion',
      matchedOn: 'name',
      score: 1,
    });
    expect(out.total).toBe(1);
    expect(out.truncated).toBeUndefined();
    const both = unwrap(await tool.cb({ query: 'kelly' }));
    expect(both.results.map((r: { path: string }) => r.path)).toEqual([
      'Finance/Kelly notes.md',
      'Finance/Kelly criterion.md',
    ]);
  });

  it('matches the frontmatter title', async () => {
    const out = unwrap(await tool.cb({ query: 'betting sizes' }));
    expect(out.results[0]).toMatchObject({ path: 'Finance/Kelly notes.md', matchedOn: 'title', score: 1 });
  });

  it('matches an alias and names it', async () => {
    const out = unwrap(await tool.cb({ query: 'bobby tables' }));
    expect(out.results[0]).toMatchObject({
      path: 'People/Bob Smith.md',
      matchedOn: 'alias',
      alias: 'Bobby Tables',
      score: 1,
    });
  });

  it('an exact alias outranks a prefix match on another name', async () => {
    const out = unwrap(await tool.cb({ query: 'robert' }));
    expect(out.results[0]).toMatchObject({ path: 'People/Bob Smith.md', matchedOn: 'alias' });
    expect(out.results[1]).toMatchObject({ path: 'People/Robert Jones.md', matchedOn: 'name' });
  });

  it('matches non-contiguous characters, case-insensitively', async () => {
    const out = unwrap(await tool.cb({ query: 'KLCRT' }));
    expect(out.results[0].path).toBe('Finance/Kelly criterion.md');
  });

  it('never returns stub notes for unresolved links', async () => {
    const out = unwrap(await tool.cb({ query: 'missing note' }));
    expect(out.results).toEqual([]);
    expect(out.total).toBe(0);
  });

  it('folder narrows the candidates; a trailing slash is fine', async () => {
    const out = unwrap(await tool.cb({ query: 'rob', folder: 'People/' }));
    expect(out.results.every((r: { path: string }) => r.path.startsWith('People/'))).toBe(true);
    expect(out.results.length).toBeGreaterThan(0);
  });

  it('the vault root is the same as no folder', async () => {
    const all = unwrap(await tool.cb({ query: 'rob' }));
    const root = unwrap(await tool.cb({ query: 'rob', folder: '/' }));
    expect(root).toEqual(all);
  });

  it('limit truncates and total keeps the full count', async () => {
    const out = unwrap(await tool.cb({ query: 'o', limit: 2 }));
    expect(out.results).toHaveLength(2);
    expect(out.total).toBeGreaterThan(2);
    expect(out.truncated).toBe(true);
  });

  it('an unknown folder is an error', async () => {
    expect(errorText(await tool.cb({ query: 'x', folder: 'Nope' }))).toMatch(/Folder not found/);
  });

  it('refuses folders that leave the vault', async () => {
    expect(errorText(await tool.cb({ query: 'x', folder: '../etc' }))).toMatch(/inside the vault/);
    expect(errorText(await tool.cb({ query: 'x', folder: '/etc' }))).toMatch(/vault-relative/);
  });

  it('schema rejects an empty query and an out-of-range limit', () => {
    expect(acceptsArgs(tool, { query: '' })).toBe(false);
    expect(acceptsArgs(tool, { query: 'a', limit: 101 })).toBe(false);
    expect(acceptsArgs(tool, { query: 'a', limit: 100 })).toBe(true);
  });
});
