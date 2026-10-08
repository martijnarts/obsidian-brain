import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdtemp, writeFile, readFile, rm, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { openDb, type DatabaseHandle } from '../../src/store/db.js';
import { upsertNode } from '../../src/store/nodes.js';
import { registerSetTaskStatusTool } from '../../src/tools/set-task-status.js';
import type { ServerContext } from '../../src/context.js';
import { makeMockServer, unwrap, type RecordedTool } from '../helpers/mock-server.js';

const NOTE = [
  '---',
  'tags: [todo]',
  '---',
  '# Todo',
  '',
  '- [ ] Buy milk  ',
  '\t* [x] Call 🐙 back',
  '3. [/] Half done',
  'Plain line',
  '```',
  '- [ ] fenced',
  '```',
  '',
].join('\n');

describe('set_task_status', () => {
  let vault: string;
  let db: DatabaseHandle;
  let tool: RecordedTool;
  let reindexes: number;

  beforeEach(async () => {
    vault = await mkdtemp(join(tmpdir(), 'kg-set-task-'));
    db = openDb(':memory:');
    await writeFile(join(vault, 'Todo.md'), NOTE);
    upsertNode(db, { id: 'Todo.md', title: 'Todo', content: '', frontmatter: {} });
    reindexes = 0;
    const ctx = {
      db,
      config: { vaultPath: vault },
      enqueueBackgroundReindex: () => { reindexes++; },
    } as unknown as ServerContext;
    const { server, registered } = makeMockServer();
    registerSetTaskStatusTool(server, ctx);
    tool = registered.find((t) => t.name === 'set_task_status')!;
  });

  afterEach(async () => {
    db.close();
    await rm(vault, { recursive: true, force: true });
  });

  const read = (): Promise<string> => readFile(join(vault, 'Todo.md'), 'utf-8');

  it('done ticks the box and leaves every other byte alone', async () => {
    const out = unwrap(await tool.cb({ name: 'Todo', line: 6, status: 'done' }));
    expect(out).toEqual({
      path: 'Todo.md', line: 6, previousStatus: ' ', status: 'x', state: 'done', text: 'Buy milk', changed: true,
    });
    expect(await read()).toBe(NOTE.replace('- [ ] Buy milk  ', '- [x] Buy milk  '));
    expect(reindexes).toBe(1);
    expect(await readdir(vault)).toEqual(['Todo.md']);
  });

  it('open unticks a tab-indented star task', async () => {
    unwrap(await tool.cb({ name: 'Todo', line: 7, status: 'open' }));
    expect(await read()).toBe(NOTE.replace('\t* [x] Call', '\t* [ ] Call'));
  });

  it('writes a raw status character on an ordered task', async () => {
    const out = unwrap(await tool.cb({ name: 'Todo.md', line: 8, status: '-' }));
    expect(out.state).toBe('cancelled');
    expect(out.previousStatus).toBe('/');
    expect(await read()).toBe(NOTE.replace('3. [/]', '3. [-]'));
    unwrap(await tool.cb({ name: 'Todo', line: 8, status: '🔥' }));
    expect(await read()).toBe(NOTE.replace('3. [/]', '3. [🔥]'));
  });

  it('keeps CRLF line endings', async () => {
    const crlf = NOTE.replace(/\n/g, '\r\n');
    await writeFile(join(vault, 'Todo.md'), crlf);
    unwrap(await tool.cb({ name: 'Todo', line: 6, status: 'done' }));
    expect(await read()).toBe(crlf.replace('- [ ] Buy', '- [x] Buy'));
  });

  it('does not write when the task already has the status', async () => {
    const out = unwrap(await tool.cb({ name: 'Todo', line: 7, status: 'x' }));
    expect(out.changed).toBe(false);
    expect(reindexes).toBe(0);
    expect(await read()).toBe(NOTE);
  });

  it('expectedText guards against a stale line', async () => {
    unwrap(await tool.cb({ name: 'Todo', line: 6, status: 'done', expectedText: ' Buy milk ' }));
    const stale = await tool.cb({ name: 'Todo', line: 7, status: 'open', expectedText: 'Buy milk' });
    expect(stale.isError).toBe(true);
    expect(stale.content[0].text).toMatch(/is "Call 🐙 back", not "Buy milk"/);
    expect(await read()).toBe(NOTE.replace('- [ ] Buy', '- [x] Buy'));
  });

  it('refuses non-task lines, fenced and frontmatter lines, and lines past the end', async () => {
    for (const line of [2, 4, 9, 11]) {
      const res = await tool.cb({ name: 'Todo', line, status: 'done' });
      expect(res.isError).toBe(true);
      expect(res.content[0].text).toMatch(new RegExp(`Line ${line} of "Todo.md" is not a task`));
    }
    const past = await tool.cb({ name: 'Todo', line: 99, status: 'done' });
    expect(past.content[0].text).toMatch(/past the end .*13 lines/);
    expect(await read()).toBe(NOTE);
  });

  it('rejects a bad status before touching the file', async () => {
    for (const status of ['', 'xx', ']', '[', '\n']) {
      const res = await tool.cb({ name: 'Todo', line: 6, status });
      expect(res.isError).toBe(true);
      expect(res.content[0].text).toMatch(/status must be/);
    }
  });

  it('errors on a missing note', async () => {
    const res = await tool.cb({ name: 'Nope', line: 1, status: 'done' });
    expect(res.content[0].text).toMatch(/No note found/);
  });
});
