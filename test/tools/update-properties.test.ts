import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdtemp, mkdir, writeFile, readFile, rm, symlink } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { openDb, type DatabaseHandle } from '../../src/store/db.js';
import { upsertNode } from '../../src/store/nodes.js';
import { registerUpdatePropertiesTool } from '../../src/tools/update-properties.js';
import type { ServerContext } from '../../src/context.js';
import { makeMockServer, unwrap } from '../helpers/mock-server.js';

describe('tools/update_properties', () => {
  let vault: string;
  let outside: string;
  let db: DatabaseHandle;
  let reindexes: number;

  beforeEach(async () => {
    vault = await mkdtemp(join(tmpdir(), 'kg-props-'));
    outside = await mkdtemp(join(tmpdir(), 'kg-props-outside-'));
    db = openDb(':memory:');
    reindexes = 0;
  });

  afterEach(async () => {
    db.close();
    await rm(vault, { recursive: true, force: true });
    await rm(outside, { recursive: true, force: true });
  });

  function ctx(): ServerContext {
    return {
      db,
      config: { vaultPath: vault },
      ensureEmbedderReady: async () => {},
      pipeline: { index: async () => undefined },
      enqueueBackgroundReindex: () => {
        reindexes++;
      },
    } as unknown as ServerContext;
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  async function raw(args: Record<string, unknown>): Promise<any> {
    const { server, registered } = makeMockServer();
    registerUpdatePropertiesTool(server, ctx());
    return registered[0].cb(args);
  }

  async function note(rel: string, content: string, title = rel.replace(/\.md$/, '')): Promise<void> {
    await writeFile(join(vault, rel), content, 'utf-8');
    upsertNode(db, { id: rel, title, content: '', frontmatter: {} });
  }

  const body = '# Heading\n\nBody text with [[link]].\n';

  it('sets and removes several keys in one write, keeping the body and other keys', async () => {
    await note('n.md', `---\ntitle: Note\nstatus: draft\nold: 1\ntags:\n  - a\n---\n${body}`);
    const out = unwrap(await raw({ name: 'n.md', set: { status: 'done', rating: 4, tags: ['a', 'b'] }, remove: ['old'] }));

    expect(out).toEqual({
      path: 'n.md',
      set: ['status', 'rating', 'tags'],
      removed: ['old'],
      frontmatter: { title: 'Note', status: 'done', tags: ['a', 'b'], rating: 4 },
    });
    expect(await readFile(join(vault, 'n.md'), 'utf-8')).toBe(
      `---\ntitle: Note\nstatus: done\ntags:\n  - a\n  - b\nrating: 4\n---\n${body}`,
    );
    expect(reindexes).toBe(1);
  });

  it('creates the frontmatter block when the note has none', async () => {
    await note('plain.md', body);
    unwrap(await raw({ name: 'plain', set: { status: 'new' } }));
    expect(await readFile(join(vault, 'plain.md'), 'utf-8')).toBe(`---\nstatus: new\n---\n${body}`);
  });

  it('removing the last key drops the block', async () => {
    await note('one.md', `---\nk: v\n---\n${body}`);
    unwrap(await raw({ name: 'one.md', remove: ['k'] }));
    expect(await readFile(join(vault, 'one.md'), 'utf-8')).toBe(body);
  });

  it('a null value in set removes the key; set wins over remove for the same key', async () => {
    await note('n.md', `---\na: 1\nb: 2\n---\n${body}`);
    const out = unwrap(await raw({ name: 'n.md', set: { a: null, b: 3 }, remove: ['b'] }));
    expect(out.removed).toEqual(['a']);
    expect(out.set).toEqual(['b']);
    expect(out.frontmatter).toEqual({ b: 3 });
  });

  it('reports absent keys and leaves the file untouched when nothing changes', async () => {
    const original = `---\nz:   'spaced'\n---\n${body}`;
    await note('n.md', original);
    const out = unwrap(await raw({ name: 'n.md', remove: ['missing'] }));
    expect(out.notPresent).toEqual(['missing']);
    expect(out.removed).toEqual([]);
    expect(await readFile(join(vault, 'n.md'), 'utf-8')).toBe(original);
    expect(reindexes).toBe(0);
  });

  it('dryRun returns the new frontmatter and does not write', async () => {
    const original = `---\na: 1\n---\n${body}`;
    await note('n.md', original);
    const out = unwrap(await raw({ name: 'n.md', set: { b: true }, dryRun: true }));
    expect(out.dryRun).toBe(true);
    expect(out.frontmatter).toEqual({ a: 1, b: true });
    expect(await readFile(join(vault, 'n.md'), 'utf-8')).toBe(original);
    expect(reindexes).toBe(0);
  });

  it('resolves a note by title', async () => {
    await mkdir(join(vault, 'Sub'));
    await note('Sub/Long name.md', body, 'Long name');
    unwrap(await raw({ name: 'Long name', set: { x: 1 } }));
    expect(await readFile(join(vault, 'Sub/Long name.md'), 'utf-8')).toContain('x: 1');
  });

  it('rejects calls with nothing to do and invalid keys', async () => {
    await note('n.md', body);
    let res = await raw({ name: 'n.md' });
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toMatch(/Nothing to do/);
    res = await raw({ name: 'n.md', set: {}, remove: [] });
    expect(res.isError).toBe(true);
    res = await raw({ name: 'n.md', set: { ' ': 1 } });
    expect(res.content[0].text).toMatch(/Invalid frontmatter key/);
    res = await raw({ name: 'n.md', remove: ['a\nb'] });
    expect(res.content[0].text).toMatch(/Invalid frontmatter key/);
  });

  it('errors on a missing note, an ambiguous name, and a stub', async () => {
    await note('Alpha one.md', body, 'Alpha one');
    await note('Alpha two.md', body, 'Alpha two');
    upsertNode(db, { id: '_stub/Ghost.md', title: 'Ghost', content: '', frontmatter: { _stub: true } });

    let res = await raw({ name: 'nothing here', set: { a: 1 } });
    expect(res.content[0].text).toMatch(/No note found/);
    res = await raw({ name: 'alpha', set: { a: 1 } });
    expect(res.content[0].text).toMatch(/Multiple notes match/);
    res = await raw({ name: '_stub/Ghost.md', set: { a: 1 } });
    expect(res.content[0].text).toMatch(/unresolved link target/);
  });

  it('refuses a note whose file is a symlink out of the vault', async () => {
    const target = join(outside, 'secret.md');
    await writeFile(target, body, 'utf-8');
    await symlink(target, join(vault, 'link.md'));
    upsertNode(db, { id: 'link.md', title: 'link', content: '', frontmatter: {} });

    const res = await raw({ name: 'link.md', set: { a: 1 } });
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toMatch(/escapes the vault/);
    expect(await readFile(target, 'utf-8')).toBe(body);
  });

  it('refuses an indexed id that climbs out of the vault', async () => {
    await writeFile(join(outside, 'x.md'), body, 'utf-8');
    const rel = `../${outside.split('/').pop()}/x.md`;
    upsertNode(db, { id: rel, title: 'x', content: '', frontmatter: {} });
    const res = await raw({ name: rel, set: { a: 1 } });
    expect(res.content[0].text).toMatch(/escapes the vault/);
  });
});
