import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdtemp, rm, utimes } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { openDb, type DatabaseHandle } from '../../src/store/db.js';
import { upsertNode } from '../../src/store/nodes.js';
import { noteTags, registerQueryNotesTool } from '../../src/tools/query-notes.js';
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

const paths = (out: { results: Array<{ path: string }> }): string[] => out.results.map((r) => r.path);

describe('noteTags', () => {
  it('merges frontmatter and inline tags, strips #, dedupes case-insensitively', () => {
    expect(noteTags({ tags: ['Book', '#fiction'], inline_tags: ['book', 'todo'] })).toEqual(['Book', 'fiction', 'todo']);
  });

  it('splits a string on commas and whitespace and reads the `tag` key', () => {
    expect(noteTags({ tags: 'a, b c' })).toEqual(['a', 'b', 'c']);
    expect(noteTags({ tag: 'solo' })).toEqual(['solo']);
    expect(noteTags({ tags: [1, null, ''] })).toEqual([]);
  });
});

describe('tools/query_notes', () => {
  let vault: string;
  let db: DatabaseHandle;
  let tool: SchemaTool;

  beforeEach(async () => {
    vault = await mkdtemp(join(tmpdir(), 'kg-query-'));
    db = openDb(':memory:');
    await writeVault(vault, {
      'Books/Dune.md': '---\ntags: [Book, scifi]\nrating: 5\nstatus: read\nauthor: Herbert\n---\nSee [[Unwritten]].\n',
      'Books/Emma.md': '---\ntags: book\nrating: 3\nstatus: reading\n---\nbody #Classic\n',
      'Books/Notes/Index.md': '---\ntitle: Book index\n---\nrating:: 4\n',
      'Areas/Work.md': '---\ntags: [area/work]\n---\nbody\n',
      'Root.md': 'plain note\n',
    });
    await indexVault(db, vault);
    await utimes(join(vault, 'Root.md'), new Date('2020-01-01T00:00:00Z'), new Date('2020-01-01T00:00:00Z'));
    await utimes(join(vault, 'Books/Dune.md'), new Date('2024-06-01T00:00:00Z'), new Date('2024-06-01T00:00:00Z'));
    const { server, registered } = makeSchemaServer();
    registerQueryNotesTool(server, buildCtx(db, vault));
    tool = registered[0]!;
  });

  afterEach(async () => {
    db.close();
    await rm(vault, { recursive: true, force: true });
  });

  it('filters on a frontmatter value and returns the note record', async () => {
    const out = unwrap(await tool.cb({ filter: { '==': [{ var: 'frontmatter.status' }, 'read'] } }));
    expect(out.total).toBe(1);
    expect(out.results[0]).toEqual({
      path: 'Books/Dune.md',
      title: 'Dune',
      tags: ['Book', 'scifi'],
      mtime: '2024-06-01T00:00:00.000Z',
      size: expect.any(Number),
      frontmatter: { tags: ['Book', 'scifi'], rating: 5, status: 'read', author: 'Herbert' },
    });
  });

  it('matches every real note with a constant-true filter, never stubs, sorted by path', async () => {
    const out = unwrap(await tool.cb({ filter: { '==': [1, 1] } }));
    expect(paths(out)).toEqual(['Areas/Work.md', 'Books/Dune.md', 'Books/Emma.md', 'Books/Notes/Index.md', 'Root.md']);
  });

  it('has_tag is case-insensitive, ignores #, includes inline tags and nested tags', async () => {
    const book = unwrap(await tool.cb({ filter: { has_tag: [{ var: 'tags' }, '#BOOK'] } }));
    expect(paths(book)).toEqual(['Books/Dune.md', 'Books/Emma.md']);
    const inline = unwrap(await tool.cb({ filter: { has_tag: [{ var: 'tags' }, 'classic'] } }));
    expect(paths(inline)).toEqual(['Books/Emma.md']);
    const nested = unwrap(await tool.cb({ filter: { has_tag: [{ var: 'tags' }, 'area'] } }));
    expect(paths(nested)).toEqual(['Areas/Work.md']);
    const bad = unwrap(await tool.cb({ filter: { has_tag: [{ var: 'title' }, 'x'] } }));
    expect(bad.total).toBe(0);
  });

  it('exposes path, folder, name, title, mtime and size to the filter', async () => {
    const folder = unwrap(await tool.cb({ filter: { '==': [{ var: 'folder' }, 'Books/Notes'] } }));
    expect(paths(folder)).toEqual(['Books/Notes/Index.md']);
    const root = unwrap(await tool.cb({ filter: { '==': [{ var: 'folder' }, ''] } }));
    expect(paths(root)).toEqual(['Root.md']);
    const name = unwrap(await tool.cb({ filter: { '==': [{ var: 'name' }, 'Index'] } }));
    expect(name.results[0].title).toBe('Book index');
    const under = unwrap(await tool.cb({ filter: { in: ['Books/', { var: 'path' }] } }));
    expect(under.total).toBe(3);
    const old = unwrap(await tool.cb({ filter: { '<': [{ var: 'mtime' }, '2021'] } }));
    expect(paths(old)).toEqual(['Root.md']);
    const sized = unwrap(await tool.cb({ filter: { '>': [{ var: 'size' }, 0] } }));
    expect(sized.total).toBe(5);
  });

  it('sees Dataview inline fields in frontmatter but not the inline_tags index field', async () => {
    const out = unwrap(await tool.cb({ filter: { '==': [{ var: 'frontmatter.rating' }, '4'] } }));
    expect(paths(out)).toEqual(['Books/Notes/Index.md']);
    const emma = unwrap(await tool.cb({ filter: { '==': [{ var: 'name' }, 'Emma'] } }));
    expect(emma.results[0].frontmatter.inline_tags).toBeUndefined();
    expect(emma.results[0].tags).toEqual(['book', 'Classic']);
  });

  it('combines conditions with and/or', async () => {
    const out = unwrap(
      await tool.cb({
        filter: {
          and: [{ has_tag: [{ var: 'tags' }, 'book'] }, { '>=': [{ var: 'frontmatter.rating' }, 4] }],
        },
      }),
    );
    expect(paths(out)).toEqual(['Books/Dune.md']);
  });

  it('fields projects frontmatter keys, skipping absent ones', async () => {
    const out = unwrap(await tool.cb({ filter: { has_tag: [{ var: 'tags' }, 'book'] }, fields: ['rating', 'nope'] }));
    expect(out.results.map((r: { frontmatter: unknown }) => r.frontmatter)).toEqual([{ rating: 5 }, { rating: 3 }]);
  });

  it('sorts by a frontmatter key, numbers numerically, missing values last in both orders', async () => {
    const all = { '==': [1, 1] };
    const asc = unwrap(await tool.cb({ filter: all, sort: { field: 'frontmatter.rating' } }));
    // Index has rating "4" (a string from an inline field); strings and numbers compare as text.
    expect(paths(asc).slice(0, 3)).toEqual(['Books/Emma.md', 'Books/Notes/Index.md', 'Books/Dune.md']);
    expect(paths(asc).slice(3)).toEqual(['Areas/Work.md', 'Root.md']);
    const desc = unwrap(await tool.cb({ filter: all, sort: { field: 'frontmatter.rating', order: 'desc' } }));
    expect(paths(desc)).toEqual(['Books/Dune.md', 'Books/Notes/Index.md', 'Books/Emma.md', 'Areas/Work.md', 'Root.md']);
  });

  it('sorts numbers numerically', async () => {
    const out = unwrap(
      await tool.cb({ filter: { has_tag: [{ var: 'tags' }, 'book'] }, sort: { field: 'frontmatter.rating', order: 'asc' } }),
    );
    expect(paths(out)).toEqual(['Books/Emma.md', 'Books/Dune.md']);
  });

  it('sorts by title and by mtime', async () => {
    const all = { '==': [1, 1] };
    const byTitle = unwrap(await tool.cb({ filter: all, sort: { field: 'title' } }));
    expect(byTitle.results[0].title).toBe('Book index');
    const newest = unwrap(await tool.cb({ filter: all, sort: { field: 'mtime', order: 'desc' }, limit: 5 }));
    expect(paths(newest).at(-1)).toBe('Root.md');
  });

  it('limit truncates and total keeps the full count', async () => {
    const out = unwrap(await tool.cb({ filter: { '==': [1, 1] }, limit: 2 }));
    expect(out.results).toHaveLength(2);
    expect(out.total).toBe(5);
    expect(out.truncated).toBe(true);
  });

  it('skips notes that the index has but the disk no longer does', async () => {
    upsertNode(db, { id: 'Gone.md', title: 'Gone', content: '', frontmatter: {} });
    const out = unwrap(await tool.cb({ filter: { '==': [1, 1] } }));
    expect(paths(out)).not.toContain('Gone.md');
  });

  it('rejects a filter that is not a single-operator JsonLogic object', async () => {
    expect(errorText(await tool.cb({ filter: {} }))).toMatch(/exactly one operator/);
    expect(errorText(await tool.cb({ filter: { a: 1, b: 2 } }))).toMatch(/exactly one operator/);
    expect(errorText(await tool.cb({ filter: [1] }))).toMatch(/exactly one operator/);
    expect(errorText(await tool.cb({ filter: null }))).toMatch(/exactly one operator/);
  });

  it('reports an unknown operator clearly', async () => {
    expect(errorText(await tool.cb({ filter: { startsWith: [{ var: 'path' }, 'Books'] } }))).toMatch(
      /Invalid filter: Unrecognized operation startsWith/,
    );
  });

  it('rejects an unknown sort field', async () => {
    expect(errorText(await tool.cb({ filter: { '==': [1, 1] }, sort: { field: 'size' } }))).toMatch(/sort.field/);
    expect(errorText(await tool.cb({ filter: { '==': [1, 1] }, sort: { field: 'frontmatter.' } }))).toMatch(/sort.field/);
  });

  it('schema requires filter and bounds limit', () => {
    expect(acceptsArgs(tool, {})).toBe(false);
    expect(acceptsArgs(tool, { filter: { '==': [1, 1] }, limit: 501 })).toBe(false);
    expect(acceptsArgs(tool, { filter: { '==': [1, 1] }, sort: { field: 'path', order: 'up' } })).toBe(false);
    expect(acceptsArgs(tool, { filter: { '==': [1, 1] }, limit: 500, fields: ['a'] })).toBe(true);
  });
});
