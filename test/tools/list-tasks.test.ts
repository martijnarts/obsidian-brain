import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdtemp, mkdir, writeFile, rm, symlink } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { openDb, type DatabaseHandle } from '../../src/store/db.js';
import { upsertNode } from '../../src/store/nodes.js';
import { registerListTasksTool } from '../../src/tools/list-tasks.js';
import type { ServerContext } from '../../src/context.js';
import { makeMockServer, unwrap, type RecordedTool } from '../helpers/mock-server.js';

const PROJECT = [
  '---',
  'title: Project',
  '- [ ] not a task, frontmatter',
  '---',
  '# Project',
  '',
  '- [ ] Write spec 📅 2026-10-12',
  '  - [x] Draft outline',
  '  - plain bullet',
  '    - [/] Nested under a bullet',
  '* [-] Dropped idea',
  '1. [X] Numbered done',
  '',
  '```md',
  '- [ ] inside a fence',
  '```',
  '',
  'Paragraph text.',
  '\t- [ ] tab-indented after a paragraph',
  '- [?] question',
  '- [ ]',
  '-[ ] no space is not a task',
  '- [ ]no space after bracket is not a task',
].join('\n');

describe('list_tasks', () => {
  let vault: string;
  let db: DatabaseHandle;
  let tool: RecordedTool;

  beforeEach(async () => {
    vault = await mkdtemp(join(tmpdir(), 'kg-list-tasks-'));
    db = openDb(':memory:');
    await mkdir(join(vault, 'Work', 'Sub'), { recursive: true });
    await mkdir(join(vault, '.obsidian'), { recursive: true });
    await writeFile(join(vault, 'Work', 'Project.md'), PROJECT);
    await writeFile(join(vault, 'Work', 'Sub', 'Deep.md'), '- [ ] deep task\r\n- [x] deep done\r\n');
    await writeFile(join(vault, 'Inbox.md'), '- [ ] inbox one\n- [ ] inbox two\n- [ ] inbox three\n');
    await writeFile(join(vault, '.obsidian', 'Hidden.md'), '- [ ] hidden\n');
    upsertNode(db, { id: 'Work/Project.md', title: 'Project', content: '', frontmatter: {} });
    upsertNode(db, { id: 'Inbox.md', title: 'Inbox', content: '', frontmatter: {} });
    upsertNode(db, { id: 'Work/Plan A.md', title: 'Plan A', content: '', frontmatter: {} });
    upsertNode(db, { id: 'Work/Plan B.md', title: 'Plan B', content: '', frontmatter: {} });
    const { server, registered } = makeMockServer();
    registerListTasksTool(server, { db, config: { vaultPath: vault } } as unknown as ServerContext);
    tool = registered.find((t) => t.name === 'list_tasks')!;
  });

  afterEach(async () => {
    db.close();
    await rm(vault, { recursive: true, force: true });
  });

  it('parses every task shape of one note with line, status, indent, parent and due', async () => {
    const out = unwrap(await tool.cb({ name: 'Project', status: 'all' }));
    expect(out.total).toBe(8);
    expect(out.truncated).toBe(false);
    expect(out.tasks).toEqual([
      { path: 'Work/Project.md', line: 7, status: ' ', state: 'open', text: 'Write spec 📅 2026-10-12', indent: 0, due: '2026-10-12' },
      { path: 'Work/Project.md', line: 8, status: 'x', state: 'done', text: 'Draft outline', indent: 1, parentLine: 7 },
      { path: 'Work/Project.md', line: 10, status: '/', state: 'open', text: 'Nested under a bullet', indent: 2, parentLine: 7 },
      { path: 'Work/Project.md', line: 11, status: '-', state: 'cancelled', text: 'Dropped idea', indent: 0 },
      { path: 'Work/Project.md', line: 12, status: 'X', state: 'done', text: 'Numbered done', indent: 0 },
      { path: 'Work/Project.md', line: 19, status: ' ', state: 'open', text: 'tab-indented after a paragraph', indent: 0 },
      { path: 'Work/Project.md', line: 20, status: '?', state: 'open', text: 'question', indent: 0 },
      { path: 'Work/Project.md', line: 21, status: ' ', state: 'open', text: '', indent: 0 },
    ]);
  });

  it('defaults to open tasks; done picks x and X', async () => {
    const open = unwrap(await tool.cb({ name: 'Work/Project.md' }));
    expect(open.tasks.map((t: { line: number }) => t.line)).toEqual([7, 10, 19, 20, 21]);
    const done = unwrap(await tool.cb({ name: 'Project', status: 'done' }));
    expect(done.tasks.map((t: { line: number }) => t.line)).toEqual([8, 12]);
  });

  it('scans the whole vault in path order, skips hidden folders, handles CRLF', async () => {
    const out = unwrap(await tool.cb({ status: 'all' }));
    const paths = [...new Set(out.tasks.map((t: { path: string }) => t.path))];
    expect(paths).toEqual(['Inbox.md', 'Work/Project.md', 'Work/Sub/Deep.md']);
    const deep = out.tasks.filter((t: { path: string }) => t.path === 'Work/Sub/Deep.md');
    expect(deep.map((t: { text: string }) => t.text)).toEqual(['deep task', 'deep done']);
  });

  it('scopes to a folder, with or without slashes', async () => {
    for (const folder of ['Work/Sub', '/Work/Sub/', './Work/Sub']) {
      const out = unwrap(await tool.cb({ folder, status: 'all' }));
      expect(out.tasks.map((t: { path: string }) => t.path)).toEqual(['Work/Sub/Deep.md', 'Work/Sub/Deep.md']);
    }
    const root = unwrap(await tool.cb({ folder: '.', status: 'all' }));
    expect(root.total).toBe(13);
  });

  it('pages with limit and offset', async () => {
    const first = unwrap(await tool.cb({ folder: '', limit: 2 }));
    expect(first.total).toBe(9);
    expect(first.truncated).toBe(true);
    expect(first.tasks.map((t: { text: string }) => t.text)).toEqual(['inbox one', 'inbox two']);
    const second = unwrap(await tool.cb({ limit: 2, offset: 2 }));
    expect(second.offset).toBe(2);
    expect(second.tasks.map((t: { text: string }) => t.text)).toEqual(['inbox three', 'Write spec 📅 2026-10-12']);
    const past = unwrap(await tool.cb({ offset: 50 }));
    expect(past.tasks).toEqual([]);
    expect(past.truncated).toBe(false);
  });

  it('rejects name with folder, bad folders, and escapes', async () => {
    const both = await tool.cb({ name: 'Inbox', folder: 'Work' });
    expect(both.isError).toBe(true);
    expect(both.content[0].text).toMatch(/not both/);
    expect((await tool.cb({ folder: 'Nope' })).content[0].text).toMatch(/Folder not found/);
    expect((await tool.cb({ folder: 'Inbox.md' })).content[0].text).toMatch(/Folder not found/);
    expect((await tool.cb({ folder: '.obsidian' })).content[0].text).toMatch(/Hidden folders/);
    expect((await tool.cb({ folder: '../..' })).content[0].text).toMatch(/outside the vault/);
  });

  it('errors on a missing, ambiguous or unreadable note', async () => {
    expect((await tool.cb({ name: 'Nothing like it' })).content[0].text).toMatch(/No note found/);
    expect((await tool.cb({ name: 'Plan' })).content[0].text).toMatch(/Multiple notes match/);
    expect((await tool.cb({ name: 'Plan A' })).content[0].text).toMatch(/Could not read "Work\/Plan A.md"/);
  });

  it('skips a symlink that points out of the vault', async () => {
    const outside = await mkdtemp(join(tmpdir(), 'kg-list-tasks-outside-'));
    try {
      await writeFile(join(outside, 'Secret.md'), '- [ ] secret\n');
      await symlink(join(outside, 'Secret.md'), join(vault, 'Linked.md'));
      const out = unwrap(await tool.cb({ status: 'all' }));
      expect(out.tasks.some((t: { text: string }) => t.text === 'secret')).toBe(false);

      upsertNode(db, { id: 'Linked.md', title: 'Linked', content: '', frontmatter: {} });
      expect((await tool.cb({ name: 'Linked' })).content[0].text).toMatch(/outside the vault/);
    } finally {
      await rm(outside, { recursive: true, force: true });
    }
  });
});
