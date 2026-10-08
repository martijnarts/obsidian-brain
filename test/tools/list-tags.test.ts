import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { openDb, type DatabaseHandle } from '../../src/store/db.js';
import { upsertNode } from '../../src/store/nodes.js';
import { registerListTagsTool } from '../../src/tools/list-tags.js';
import type { ServerContext } from '../../src/context.js';
import { makeMockServer, unwrap } from '../helpers/mock-server.js';

describe('tools/list_tags', () => {
  let db: DatabaseHandle;

  beforeEach(() => {
    db = openDb(':memory:');
    upsertNode(db, { id: 'a.md', title: 'a', content: '', frontmatter: { tags: ['project/alpha', '#idea'] } });
    upsertNode(db, { id: 'b.md', title: 'b', content: '', frontmatter: { tag: 'idea', inline_tags: ['project/beta'] } });
    upsertNode(db, { id: 'c.md', title: 'c', content: '', frontmatter: { tags: 'idea, zeta' } });
    upsertNode(db, { id: '_stub/s.md', title: 's', content: '', frontmatter: { _stub: true, tags: ['ghost'] } });
  });

  afterEach(() => db.close());

  async function call(args: Record<string, unknown>) {
    const { server, registered } = makeMockServer();
    registerListTagsTool(server, { db } as ServerContext);
    return unwrap(await registered[0].cb(args));
  }

  it('counts notes per tag from frontmatter and inline tags, parents included, stubs skipped', async () => {
    const out = await call({});
    expect(out.tags).toEqual([
      { tag: 'idea', count: 3 },
      { tag: 'project', count: 2 },
      { tag: 'project/alpha', count: 1 },
      { tag: 'project/beta', count: 1 },
      { tag: 'zeta', count: 1 },
    ]);
    expect(out.totalTags).toBe(5);
    expect(out.truncated).toBe(false);
  });

  it('includeParents: false counts nested tags only under their own entry', async () => {
    const out = await call({ includeParents: false });
    expect(out.tags.map((t: { tag: string }) => t.tag)).not.toContain('project');
    expect(out.totalTags).toBe(4);
  });

  it('sort: name orders alphabetically', async () => {
    const out = await call({ sort: 'name' });
    expect(out.tags.map((t: { tag: string }) => t.tag)).toEqual([
      'idea',
      'project',
      'project/alpha',
      'project/beta',
      'zeta',
    ]);
  });

  it('prefix filters tags, with or without #', async () => {
    expect((await call({ prefix: 'project/' })).tags.map((t: { tag: string }) => t.tag)).toEqual([
      'project/alpha',
      'project/beta',
    ]);
    expect((await call({ prefix: '#zet' })).tags).toEqual([{ tag: 'zeta', count: 1 }]);
  });

  it('limit truncates and reports it', async () => {
    const out = await call({ limit: 2 });
    expect(out.tags).toHaveLength(2);
    expect(out.totalTags).toBe(5);
    expect(out.truncated).toBe(true);
  });

  it('returns an empty list for a vault without tags', async () => {
    db.close();
    db = openDb(':memory:');
    expect(await call({})).toEqual({ totalTags: 0, truncated: false, tags: [] });
  });
});
