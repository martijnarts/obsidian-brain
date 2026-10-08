import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { openDb, type DatabaseHandle } from '../../src/store/db.js';
import { upsertNode } from '../../src/store/nodes.js';
import { registerListPropertyValuesTool } from '../../src/tools/list-property-values.js';
import type { ServerContext } from '../../src/context.js';
import { makeMockServer, unwrap } from '../helpers/mock-server.js';

describe('tools/list_property_values', () => {
  let db: DatabaseHandle;

  beforeEach(() => {
    db = openDb(':memory:');
    upsertNode(db, { id: 'Work/a.md', title: 'a', content: '', frontmatter: { status: 'active', area: ['x', 'y'] } });
    upsertNode(db, { id: 'Work/b.md', title: 'b', content: '', frontmatter: { status: 'done', area: ['x'] } });
    upsertNode(db, { id: 'Home/c.md', title: 'c', content: '', frontmatter: { status: 'active', priority: 5 } });
    upsertNode(db, { id: 'Home/d.md', title: 'd', content: '', frontmatter: { priority: '5', status: null } });
    upsertNode(db, { id: 'e.md', title: 'e', content: '', frontmatter: {} });
    upsertNode(db, { id: '_stub/s.md', title: 's', content: '', frontmatter: { _stub: true, status: 'ghost' } });
  });

  afterEach(() => db.close());

  async function call(args: Record<string, unknown>) {
    const { server, registered } = makeMockServer();
    registerListPropertyValuesTool(server, { db } as ServerContext);
    return unwrap(await registered[0].cb(args));
  }

  it('counts distinct values by count desc and reports notesWithKey', async () => {
    const out = await call({ key: 'status' });
    expect(out.key).toBe('status');
    expect(out.notesWithKey).toBe(4);
    expect(out.values).toEqual([
      { value: 'active', count: 2 },
      { value: 'done', count: 1 },
      { value: null, count: 1 },
    ]);
    expect(out.totalDistinct).toBe(3);
    expect(out.truncated).toBe(false);
  });

  it('counts each element of a list value', async () => {
    const out = await call({ key: 'area' });
    expect(out.notesWithKey).toBe(2);
    expect(out.values).toEqual([
      { value: 'x', count: 2 },
      { value: 'y', count: 1 },
    ]);
  });

  it('keeps a number and a string with the same text distinct', async () => {
    const out = await call({ key: 'priority' });
    expect(out.values).toHaveLength(2);
    expect(out.values.map((v: { value: unknown }) => v.value)).toEqual(expect.arrayContaining([5, '5']));
  });

  it('folder restricts the scan, with or without a trailing slash', async () => {
    for (const folder of ['Work', 'Work/']) {
      const out = await call({ key: 'status', folder });
      expect(out.notesWithKey).toBe(2);
      expect(out.values).toEqual([
        { value: 'active', count: 1 },
        { value: 'done', count: 1 },
      ]);
    }
  });

  it('limit truncates and keeps totalDistinct', async () => {
    const out = await call({ key: 'status', limit: 1 });
    expect(out.values).toEqual([{ value: 'active', count: 2 }]);
    expect(out.totalDistinct).toBe(3);
    expect(out.truncated).toBe(true);
  });

  it('returns an empty result for an unknown key', async () => {
    expect(await call({ key: 'nope' })).toEqual({
      key: 'nope',
      notesWithKey: 0,
      totalDistinct: 0,
      truncated: false,
      values: [],
    });
  });
});
