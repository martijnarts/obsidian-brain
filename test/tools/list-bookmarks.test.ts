import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { registerListBookmarksTool } from '../../src/tools/list-bookmarks.js';
import type { ServerContext } from '../../src/context.js';
import { makeMockServer, unwrap } from '../helpers/mock-server.js';

describe('tools/list_bookmarks', () => {
  let vault: string;

  beforeEach(async () => {
    vault = await mkdtemp(join(tmpdir(), 'kg-bookmarks-'));
  });

  afterEach(async () => {
    await rm(vault, { recursive: true, force: true });
  });

  async function writeBookmarks(content: unknown): Promise<void> {
    await mkdir(join(vault, '.obsidian'), { recursive: true });
    await writeFile(
      join(vault, '.obsidian', 'bookmarks.json'),
      typeof content === 'string' ? content : JSON.stringify(content),
    );
  }

  async function call(args: Record<string, unknown> = {}) {
    const { server, registered } = makeMockServer();
    registerListBookmarksTool(server, { config: { vaultPath: vault } } as unknown as ServerContext);
    return unwrap(await registered[0].cb(args));
  }

  const sample = {
    items: [
      { type: 'file', ctime: 1, path: 'Home.md', title: 'Start' },
      { type: 'search', ctime: 2, query: 'tag:#todo' },
      {
        type: 'group',
        ctime: 3,
        title: 'Work',
        items: [
          { type: 'folder', ctime: 4, path: 'Projects' },
          { type: 'heading', ctime: 5, path: 'Plan.md', subpath: '#Goals' },
          { type: 'group', ctime: 6, title: 'Deep', items: [{ type: 'block', ctime: 7, path: 'Log.md', subpath: '#^abc' }] },
        ],
      },
      { type: 'graph', ctime: 8 },
      'junk',
    ],
  };

  it('returns an empty list with a note when the file is absent', async () => {
    const out = await call();
    expect(out.items).toEqual([]);
    expect(out.totalItems).toBe(0);
    expect(out.note).toMatch(/No bookmarks file/);
  });

  it('returns an empty list with a note when the file is malformed', async () => {
    await writeBookmarks('{not json');
    expect((await call()).note).toMatch(/not valid/);
    await writeBookmarks({ nope: true });
    expect((await call()).note).toMatch(/not valid/);
  });

  it('returns the hierarchy with known fields only and drops unknown types', async () => {
    await writeBookmarks(sample);
    const out = await call();
    expect(out.note).toBeUndefined();
    expect(out.totalItems).toBe(5);
    expect(out.items).toEqual([
      { type: 'file', path: 'Home.md', title: 'Start' },
      { type: 'search', query: 'tag:#todo' },
      {
        type: 'group',
        title: 'Work',
        items: [
          { type: 'folder', path: 'Projects' },
          { type: 'heading', path: 'Plan.md', subpath: '#Goals' },
          { type: 'group', title: 'Deep', items: [{ type: 'block', path: 'Log.md', subpath: '#^abc' }] },
        ],
      },
    ]);
  });

  it('types without group flattens group contents', async () => {
    await writeBookmarks(sample);
    const out = await call({ types: ['file', 'block'] });
    expect(out.items).toEqual([
      { type: 'file', path: 'Home.md', title: 'Start' },
      { type: 'block', path: 'Log.md', subpath: '#^abc' },
    ]);
    expect(out.totalItems).toBe(2);
  });

  it('types with group keeps groups and filters their children', async () => {
    await writeBookmarks(sample);
    const out = await call({ types: ['group', 'folder'] });
    expect(out.items).toEqual([
      {
        type: 'group',
        title: 'Work',
        items: [{ type: 'folder', path: 'Projects' }, { type: 'group', title: 'Deep', items: [] }],
      },
    ]);
    expect(out.totalItems).toBe(1);
  });

  it('a group without items gets an empty items array', async () => {
    await writeBookmarks({ items: [{ type: 'group', title: 'Empty' }] });
    expect((await call()).items).toEqual([{ type: 'group', title: 'Empty', items: [] }]);
  });
});
