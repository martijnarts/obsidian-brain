import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { openDb, type DatabaseHandle } from '../../src/store/db.js';
import { upsertNode } from '../../src/store/nodes.js';
import { setSyncMtime } from '../../src/store/sync.js';
import { registerVaultOverviewTool } from '../../src/tools/vault-overview.js';
import type { ServerContext } from '../../src/context.js';
import { makeMockServer, unwrap } from '../helpers/mock-server.js';

describe('tools/vault_overview', () => {
  let vault: string;
  let db: DatabaseHandle;

  beforeEach(async () => {
    vault = await mkdtemp(join(tmpdir(), 'kg-overview-'));
    db = openDb(':memory:');
  });

  afterEach(async () => {
    db.close();
    await rm(vault, { recursive: true, force: true });
  });

  async function call(args: Record<string, unknown> = {}) {
    const { server, registered } = makeMockServer();
    registerVaultOverviewTool(server, { db, config: { vaultPath: vault } } as unknown as ServerContext);
    return unwrap(await registered[0].cb(args));
  }

  it('returns an all-empty snapshot for an empty vault', async () => {
    expect(await call()).toEqual({ notes: 0, attachments: 0, folders: [], topTags: [], recent: [] });
  });

  it('counts notes, attachments, folders, tags and recent notes', async () => {
    await mkdir(join(vault, 'attachments'), { recursive: true });
    await mkdir(join(vault, '.obsidian'), { recursive: true });
    await writeFile(join(vault, 'attachments', 'pic.png'), 'x');
    await writeFile(join(vault, 'doc.pdf'), 'x');
    await writeFile(join(vault, 'root.md'), '# Root');
    await writeFile(join(vault, '.obsidian', 'app.json'), '{}');

    upsertNode(db, { id: 'root.md', title: 'Root', content: '', frontmatter: { tags: ['x'] } });
    upsertNode(db, { id: 'P/a.md', title: 'A', content: '', frontmatter: { tags: ['x/y'] } });
    upsertNode(db, { id: 'P/b.md', title: 'B', content: '', frontmatter: { inline_tags: ['z'] } });
    upsertNode(db, { id: 'Q/c.md', title: 'C', content: '', frontmatter: {} });
    upsertNode(db, { id: '_stub/s.md', title: 'S', content: '', frontmatter: { _stub: true } });
    setSyncMtime(db, 'root.md', 1_000);
    setSyncMtime(db, 'P/a.md', 3_000);
    setSyncMtime(db, 'P/b.md', 2_000);

    const out = await call();
    expect(out.notes).toBe(4);
    expect(out.attachments).toBe(2);
    expect(out.folders).toEqual([
      { folder: 'P', count: 2 },
      { folder: '(root)', count: 1 },
      { folder: 'Q', count: 1 },
    ]);
    expect(out.topTags).toEqual([
      { tag: 'x', count: 2 },
      { tag: 'x/y', count: 1 },
      { tag: 'z', count: 1 },
    ]);
    expect(out.recent).toEqual([
      { id: 'P/a.md', title: 'A', mtime: new Date(3_000).toISOString() },
      { id: 'P/b.md', title: 'B', mtime: new Date(2_000).toISOString() },
      { id: 'root.md', title: 'Root', mtime: new Date(1_000).toISOString() },
    ]);
  });

  it('topTags and recent limit the snapshot', async () => {
    for (let i = 0; i < 20; i++) {
      upsertNode(db, { id: `n${i}.md`, title: `n${i}`, content: '', frontmatter: { tags: [`t${i}`] } });
      setSyncMtime(db, `n${i}.md`, 1_000 + i);
    }
    let out = await call();
    expect(out.topTags).toHaveLength(15);
    expect(out.recent).toHaveLength(10);
    expect(out.recent[0].id).toBe('n19.md');

    out = await call({ topTags: 3, recent: 2 });
    expect(out.topTags).toHaveLength(3);
    expect(out.recent.map((r: { id: string }) => r.id)).toEqual(['n19.md', 'n18.md']);
  });
});
