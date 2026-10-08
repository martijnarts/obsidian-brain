import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { upsertNode } from '../../src/store/nodes.js';
import { registerListAttachmentsTool } from '../../src/tools/list-attachments.js';
import { makeHarness, loadTool, ok, err, type FileToolHarness } from '../helpers/file-tools.js';

describe('tools/list_attachments', () => {
  let h: FileToolHarness;
  let tool: ReturnType<typeof loadTool>;

  beforeEach(async () => {
    h = await makeHarness();
    await h.write('attachments/photo.png', Buffer.alloc(10));
    await h.write('attachments/Scan.PDF', Buffer.alloc(20));
    await h.write('docs/my file.pdf', Buffer.alloc(5));
    await h.write('docs/local.jpg', Buffer.alloc(1));
    await h.write('other/local.jpg', Buffer.alloc(2));
    await h.write('orphan.zip', Buffer.alloc(3));
    await h.write('Note.md', 'not an attachment');
    await h.write('.obsidian/workspace.json', '{}');
    await h.write('.hidden/secret.png', 'x');

    upsertNode(h.db, {
      id: 'Note.md',
      title: 'Note',
      content: [
        '![[photo.png]]',
        '[[photo.png|again]] counts once per note',
        '[pdf](docs/my%20file.pdf)',
        '![scan](<attachments/Scan.PDF> "title")',
        '[web](https://example.com/photo.png)',
        '`![[orphan.zip]]`',
        '```',
        '![[orphan.zip]]',
        '```',
      ].join('\n'),
      frontmatter: {},
    });
    upsertNode(h.db, {
      id: 'docs/Page.md',
      title: 'Page',
      content: '![](local.jpg) and [[attachments/photo.png#frag]] and [x](./my%20file.pdf)',
      frontmatter: {},
    });
    upsertNode(h.db, {
      id: 'other/Deep.md',
      title: 'Deep',
      content: '![[other/local.jpg]] [[Missing.png]] [[]]',
      frontmatter: {},
    });
    upsertNode(h.db, {
      id: '_stub/orphan.zip.md',
      title: 'orphan.zip',
      content: '![[orphan.zip]]',
      frontmatter: { _stub: true },
    });
    tool = loadTool(registerListAttachmentsTool, h.ctx);
  });

  afterEach(async () => {
    await h.dispose();
  });

  it('lists every non-markdown file with size and reference count, sorted by path', async () => {
    const data = ok(await tool.call({}));
    expect(data.total).toBe(6);
    expect(data.attachments).toEqual([
      { path: 'attachments/Scan.PDF', extension: 'pdf', size: 20, references: 1 },
      { path: 'attachments/photo.png', extension: 'png', size: 10, references: 2 },
      { path: 'docs/local.jpg', extension: 'jpg', size: 1, references: 1 },
      { path: 'docs/my file.pdf', extension: 'pdf', size: 5, references: 2 },
      { path: 'orphan.zip', extension: 'zip', size: 3, references: 0 },
      { path: 'other/local.jpg', extension: 'jpg', size: 2, references: 1 },
    ]);
  });

  it('unreferencedOnly keeps the orphans', async () => {
    const data = ok(await tool.call({ unreferencedOnly: true }));
    expect(data.attachments.map((a: { path: string }) => a.path)).toEqual(['orphan.zip']);
  });

  it('filters extensions case-insensitively, with or without the dot', async () => {
    const data = ok(await tool.call({ extensions: ['.PDF', 'zip'] }));
    expect(data.attachments.map((a: { path: string }) => a.path)).toEqual([
      'attachments/Scan.PDF',
      'docs/my file.pdf',
      'orphan.zip',
    ]);
  });

  it('scopes to a folder recursively, still counting references from anywhere', async () => {
    const data = ok(await tool.call({ folder: '/attachments/'.slice(1) }));
    expect(data.attachments.map((a: { path: string; references: number }) => [a.path, a.references])).toEqual([
      ['attachments/Scan.PDF', 1],
      ['attachments/photo.png', 2],
    ]);
  });

  it('pages with limit and offset', async () => {
    const data = ok(await tool.call({ limit: 2, offset: 1 }));
    expect(data).toMatchObject({ total: 6, offset: 1, limit: 2 });
    expect(data.attachments.map((a: { path: string }) => a.path)).toEqual([
      'attachments/photo.png',
      'docs/local.jpg',
    ]);
  });

  it('errors on a missing folder, a file as folder and an escape', async () => {
    expect(err(await tool.call({ folder: 'nope' }))).toMatch(/Folder not found: "nope"/);
    expect(err(await tool.call({ folder: 'orphan.zip' }))).toMatch(/Folder not found/);
    expect(err(await tool.call({ folder: '../' }))).toMatch(/outside the vault/);
    expect(err(await tool.call({ limit: 0 }))).toMatch(/invalid arguments/);
  });
});
