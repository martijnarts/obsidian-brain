import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdir, mkdtemp, rm, stat, symlink, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { getNode, upsertNode } from '../../src/store/nodes.js';
import { countEdgesBySource, countEdgesByTarget, insertEdge } from '../../src/store/edges.js';
import { registerCreateFolderTool } from '../../src/tools/create-folder.js';
import { registerDeleteFolderTool } from '../../src/tools/delete-folder.js';
import { makeHarness, loadTool, ok, err, type FileToolHarness } from '../helpers/file-tools.js';

async function exists(p: string): Promise<boolean> {
  return stat(p).then(() => true, () => false);
}

describe('tools/create_folder', () => {
  let h: FileToolHarness;
  let tool: ReturnType<typeof loadTool>;

  beforeEach(async () => {
    h = await makeHarness();
    tool = loadTool(registerCreateFolderTool, h.ctx);
  });

  afterEach(async () => {
    await h.dispose();
  });

  it('creates a nested folder chain', async () => {
    expect(ok(await tool.call({ path: 'A/B/C' }))).toEqual({ path: 'A/B/C', created: true });
    expect((await stat(join(h.vault, 'A/B/C'))).isDirectory()).toBe(true);
  });

  it('is idempotent and normalises slashes', async () => {
    ok(await tool.call({ path: 'A' }));
    expect(ok(await tool.call({ path: '/A/'.slice(1) }))).toEqual({ path: 'A', created: false });
    expect(ok(await tool.call({ path: 'A//B\\C/' }))).toEqual({ path: 'A/B/C', created: true });
  });

  it('refuses the vault root, an existing file and escapes', async () => {
    await h.write('file.txt', 'x');
    expect(err(await tool.call({ path: '' }))).toMatch(/vault root/);
    expect(err(await tool.call({ path: './' }))).toMatch(/vault root/);
    expect(err(await tool.call({ path: 'file.txt' }))).toMatch(/A file already exists at file.txt/);
    expect(err(await tool.call({ path: '../escape' }))).toMatch(/outside the vault/);
    expect(err(await tool.call({ path: 'A/../../escape' }))).toMatch(/outside the vault/);
    expect(err(await tool.call({ path: 'C:/temp' }))).toMatch(/outside the vault/);
    expect(await exists(join(h.vault, '..', 'escape'))).toBe(false);
  });

  it('refuses to create through a symlink that leaves the vault', async () => {
    const outside = await mkdtemp(join(tmpdir(), 'kg-outside-'));
    try {
      await symlink(outside, join(h.vault, 'out'));
      expect(err(await tool.call({ path: 'out/new' }))).toMatch(/outside the vault/);
      expect(await readdir(outside)).toEqual([]);
      await symlink(join(outside, 'missing'), join(h.vault, 'dangling'));
      expect(err(await tool.call({ path: 'dangling/x' }))).toMatch(/outside the vault/);
    } finally {
      await rm(outside, { recursive: true, force: true });
    }
  });
});

describe('tools/delete_folder', () => {
  let h: FileToolHarness;
  let tool: ReturnType<typeof loadTool>;

  beforeEach(async () => {
    h = await makeHarness();
    await h.write('Proj/A.md', '# A\n[[Keep]] [[Ghost]]\n');
    await h.write('Proj/Sub/B.md', '# B\n');
    await h.write('Proj/Sub/pic.png', 'png');
    await h.write('Keep.md', '# Keep\n[[A]]\n');
    upsertNode(h.db, { id: 'Proj/A.md', title: 'A', content: '', frontmatter: {} });
    upsertNode(h.db, { id: 'Proj/Sub/B.md', title: 'B', content: '', frontmatter: {} });
    upsertNode(h.db, { id: 'Proj/Stale.md', title: 'Stale', content: '', frontmatter: {} });
    upsertNode(h.db, { id: 'Keep.md', title: 'Keep', content: '', frontmatter: {} });
    upsertNode(h.db, { id: '_stub/Ghost.md', title: 'Ghost', content: '', frontmatter: { _stub: true } });
    insertEdge(h.db, { sourceId: 'Proj/A.md', targetId: 'Keep.md', context: '' });
    insertEdge(h.db, { sourceId: 'Proj/A.md', targetId: '_stub/Ghost.md', context: '' });
    insertEdge(h.db, { sourceId: 'Keep.md', targetId: 'Proj/A.md', context: '' });
    tool = loadTool(registerDeleteFolderTool, h.ctx);
  });

  afterEach(async () => {
    await h.dispose();
  });

  it('requires confirm: true', async () => {
    expect(err(await tool.call({ path: 'Proj', recursive: true }))).toMatch(/invalid arguments/);
    expect(err(await tool.call({ path: 'Proj', recursive: true, confirm: false }))).toMatch(/invalid arguments/);
    expect(await exists(join(h.vault, 'Proj'))).toBe(true);
  });

  it('deletes an empty folder without recursive', async () => {
    await mkdir(join(h.vault, 'Empty'));
    const data = ok(await tool.call({ path: 'Empty', confirm: true }));
    expect(data.deleted).toEqual({ files: 0, folders: 0, notes: 0 });
    expect(await exists(join(h.vault, 'Empty'))).toBe(false);
  });

  it('refuses a non-empty folder unless recursive', async () => {
    expect(err(await tool.call({ path: 'Proj', confirm: true }))).toMatch(
      /Proj is not empty \(3 files, 1 folders\)/,
    );
    expect(await exists(join(h.vault, 'Proj/A.md'))).toBe(true);
    expect(getNode(h.db, 'Proj/A.md')).toBeDefined();
  });

  it('dry run lists what would go and changes nothing', async () => {
    const data = ok(await tool.call({ path: 'Proj', confirm: true, recursive: true, dryRun: true }));
    expect(data).toEqual({
      dryRun: true,
      path: 'Proj',
      files: 3,
      folders: 1,
      notes: 3,
      sample: ['Proj/A.md', 'Proj/Sub', 'Proj/Sub/B.md', 'Proj/Sub/pic.png'],
      truncated: false,
    });
    expect(await exists(join(h.vault, 'Proj/Sub/B.md'))).toBe(true);
    expect(getNode(h.db, 'Proj/A.md')).toBeDefined();
    expect(h.reindexes()).toBe(0);
  });

  it('a non-recursive dry run says it would be refused', async () => {
    const data = ok(await tool.call({ path: 'Proj', confirm: true, dryRun: true }));
    expect(data.refused).toMatch(/not empty/);
  });

  it('dry run truncates the sample at 50 paths', async () => {
    for (let i = 0; i < 55; i++) await h.write(`Big/f${String(i).padStart(2, '0')}.txt`, '');
    const data = ok(await tool.call({ path: 'Big', confirm: true, recursive: true, dryRun: true }));
    expect(data.files).toBe(55);
    expect(data.sample).toHaveLength(50);
    expect(data.truncated).toBe(true);
  });

  it('recursive delete removes the files and purges every note from the index', async () => {
    const data = ok(await tool.call({ path: 'Proj/', confirm: true, recursive: true }));
    expect(data.path).toBe('Proj');
    expect(data.deleted).toEqual({ files: 3, folders: 1, notes: 3 });
    expect(data.deletedFromIndex).toEqual({ nodes: 3, edges: 2, stubsPruned: 1 });

    expect(await exists(join(h.vault, 'Proj'))).toBe(false);
    expect(await exists(join(h.vault, 'Keep.md'))).toBe(true);
    for (const id of ['Proj/A.md', 'Proj/Sub/B.md', 'Proj/Stale.md', '_stub/Ghost.md']) {
      expect(getNode(h.db, id), id).toBeUndefined();
    }
    expect(countEdgesBySource(h.db, 'Proj/A.md')).toBe(0);
    expect(countEdgesByTarget(h.db, 'Proj/A.md')).toBe(0);
    expect(getNode(h.db, 'Keep.md')).toBeDefined();

    await new Promise((r) => setTimeout(r, 0));
    expect(h.reindexes()).toBe(1);
  });

  it('refuses the root, .obsidian, a file, a missing folder, a symlink and escapes', async () => {
    await mkdir(join(h.vault, '.obsidian', 'plugins'), { recursive: true });
    await symlink(join(h.vault, 'Proj'), join(h.vault, 'alias'));
    expect(err(await tool.call({ path: '', confirm: true, recursive: true }))).toMatch(/vault root/);
    expect(err(await tool.call({ path: '/', confirm: true, recursive: true }))).toMatch(/outside the vault/);
    expect(err(await tool.call({ path: '.obsidian', confirm: true, recursive: true }))).toMatch(/\.obsidian/);
    expect(err(await tool.call({ path: '.obsidian/plugins', confirm: true, recursive: true }))).toMatch(/\.obsidian/);
    expect(err(await tool.call({ path: 'Keep.md', confirm: true }))).toMatch(/is a file/);
    expect(err(await tool.call({ path: 'Nope', confirm: true }))).toMatch(/Folder not found: "Nope"/);
    expect(err(await tool.call({ path: 'alias', confirm: true, recursive: true }))).toMatch(/symlink/);
    expect(err(await tool.call({ path: '..', confirm: true, recursive: true }))).toMatch(/outside the vault/);
    expect(await exists(join(h.vault, '.obsidian/plugins'))).toBe(true);
    expect(await exists(join(h.vault, 'Proj/A.md'))).toBe(true);
  });
});
