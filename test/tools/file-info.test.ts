import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdir, symlink, mkdtemp, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { upsertNode } from '../../src/store/nodes.js';
import { insertEdge } from '../../src/store/edges.js';
import { registerFileInfoTool } from '../../src/tools/file-info.js';
import { makeHarness, loadTool, ok, err, type FileToolHarness } from '../helpers/file-tools.js';

const NOTE = [
  '---',
  'tags: [project, "#work"]',
  '---',
  '# Title',
  'Some #inline text with #project again.',
  '## Tasks',
  '- [ ] open one',
  '- [x] done one',
  '* [X] done two',
  '1. [ ] numbered open',
  '```',
  '- [ ] not a task',
  '# not a heading',
  '```',
  '',
].join('\n');

describe('tools/file_info', () => {
  let h: FileToolHarness;
  let tool: ReturnType<typeof loadTool>;

  beforeEach(async () => {
    h = await makeHarness();
    await h.write('Projects/Plan.md', NOTE);
    await h.write('assets/pic.png', Buffer.from([1, 2, 3, 4]));
    upsertNode(h.db, { id: 'Projects/Plan.md', title: 'Plan', content: '', frontmatter: {} });
    upsertNode(h.db, { id: 'Other.md', title: 'Other', content: '', frontmatter: {} });
    insertEdge(h.db, { sourceId: 'Projects/Plan.md', targetId: 'Other.md', context: '' });
    insertEdge(h.db, { sourceId: 'Projects/Plan.md', targetId: '_stub/Missing.md', context: '' });
    insertEdge(h.db, { sourceId: 'Other.md', targetId: 'Projects/Plan.md', context: '' });
    tool = loadTool(registerFileInfoTool, h.ctx);
  });

  afterEach(async () => {
    await h.dispose();
  });

  it('describes a note: size, times, links, headings, tasks and tags', async () => {
    const data = ok(await tool.call({ path: 'Projects/Plan.md' }));
    expect(data).toMatchObject({
      path: 'Projects/Plan.md',
      kind: 'note',
      size: Buffer.byteLength(NOTE),
      indexed: true,
      links: { outgoing: 2, incoming: 1, unresolved: 1 },
      headings: 2,
      tasks: { open: 2, done: 2 },
    });
    expect(data.tags.sort()).toEqual(['inline', 'project', 'work']);
    expect(new Date(data.mtime).toISOString()).toBe(data.mtime);
    expect(new Date(data.ctime).toISOString()).toBe(data.ctime);
    expect(data).not.toHaveProperty('content');
  });

  it('reads a note the index does not know yet, with zero link counts', async () => {
    await h.write('New.md', 'tags: none\n#solo\n');
    const data = ok(await tool.call({ path: 'New.md' }));
    expect(data.indexed).toBe(false);
    expect(data.links).toEqual({ outgoing: 0, incoming: 0, unresolved: 0 });
    expect(data.tags).toEqual(['solo']);
  });

  it('counts tags from a comma-separated frontmatter string and a `tag` key', async () => {
    await h.write('Str.md', '---\ntags: "a, b c"\ntag: d\n---\nbody\n');
    expect(ok(await tool.call({ path: 'Str.md' })).tags.sort()).toEqual(['a', 'b', 'c', 'd']);
  });

  it('falls back to the whole file when the frontmatter is malformed', async () => {
    await h.write('Bad.md', '---\nkey: [unclosed\n---\n# H\n');
    const data = ok(await tool.call({ path: 'Bad.md' }));
    expect(data.kind).toBe('note');
    expect(data.tags).toEqual([]);
  });

  it('describes an attachment with its size and no note fields', async () => {
    const data = ok(await tool.call({ path: 'assets/pic.png' }));
    expect(data).toMatchObject({ path: 'assets/pic.png', kind: 'attachment', size: 4 });
    expect(data).not.toHaveProperty('links');
  });

  it('describes a folder with its child count, and the root as ""', async () => {
    await mkdir(join(h.vault, 'Projects', 'Sub'));
    expect(ok(await tool.call({ path: 'Projects/' }))).toMatchObject({ path: 'Projects', kind: 'folder', children: 2 });
    expect(ok(await tool.call({ path: '' }))).toMatchObject({ path: '', kind: 'folder' });
  });

  it('errors on a missing path', async () => {
    expect(err(await tool.call({ path: 'Nope.md' }))).toMatch(/Path not found: Nope.md/);
  });

  it('refuses paths that escape the vault', async () => {
    expect(err(await tool.call({ path: '../etc/passwd' }))).toMatch(/outside the vault/);
    expect(err(await tool.call({ path: '/etc/passwd' }))).toMatch(/outside the vault/);
    const outside = await mkdtemp(join(tmpdir(), 'kg-outside-'));
    try {
      await symlink(outside, join(h.vault, 'link'));
      expect(err(await tool.call({ path: 'link' }))).toMatch(/outside the vault/);
    } finally {
      await rm(outside, { recursive: true, force: true });
    }
  });
});
