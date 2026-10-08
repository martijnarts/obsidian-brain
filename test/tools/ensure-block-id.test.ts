import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { openDb, type DatabaseHandle } from '../../src/store/db.js';
import { upsertNode } from '../../src/store/nodes.js';
import { registerEnsureBlockIdTool } from '../../src/tools/ensure-block-id.js';
import type { ServerContext } from '../../src/context.js';
import { makeMockServer, unwrap, type RecordedTool } from '../helpers/mock-server.js';

const NOTE = [
  '---',            // 1
  'title: Notes',   // 2
  '---',            // 3
  '# Intro',        // 4
  '',               // 5
  'First line',     // 6
  'second line.',   // 7
  '',               // 8
  '- item one',     // 9
  '- item two ^two', // 10
  '',               // 11
  '| a | b |',      // 12
  '|---|---|',      // 13
  '| 1 | 2 |',      // 14
  'After table.',   // 15
  '',               // 16
  '```js',          // 17
  'const x = 1;',   // 18
  '```',            // 19
  '',               // 20
  '> [!note]',      // 21
  '> callout body', // 22
  '',               // 23
  '^callout',       // 24
  '',               // 25
  '## Twice',       // 26
  '## Twice',       // 27
  'Last line',      // 28
].join('\n');

describe('ensure_block_id', () => {
  let vault: string;
  let db: DatabaseHandle;
  let tool: RecordedTool;
  let reindexes: number;

  beforeEach(async () => {
    vault = await mkdtemp(join(tmpdir(), 'kg-block-id-'));
    db = openDb(':memory:');
    await writeFile(join(vault, 'Notes.md'), NOTE);
    upsertNode(db, { id: 'Notes.md', title: 'Notes', content: '', frontmatter: {} });
    reindexes = 0;
    const ctx = {
      db,
      config: { vaultPath: vault },
      enqueueBackgroundReindex: () => { reindexes++; },
    } as unknown as ServerContext;
    const { server, registered } = makeMockServer();
    registerEnsureBlockIdTool(server, ctx);
    tool = registered.find((t) => t.name === 'ensure_block_id')!;
  });

  afterEach(async () => {
    db.close();
    await rm(vault, { recursive: true, force: true });
  });

  const lines = async (file = 'Notes.md'): Promise<string[]> =>
    (await readFile(join(vault, file), 'utf-8')).split('\n');

  it('appends the id to the last line of a paragraph and returns a link', async () => {
    const out = unwrap(await tool.cb({ name: 'Notes', line: 6, id: 'intro-para' }));
    expect(out).toMatchObject({
      path: 'Notes.md', id: 'intro-para', created: true, dryRun: false, line: 7, link: '[[Notes#^intro-para]]',
    });
    expect(out.diff).toContain('+second line. ^intro-para');
    const after = await lines();
    expect(after[5]).toBe('First line');
    expect(after[6]).toBe('second line. ^intro-para');
    expect(after.length).toBe(NOTE.split('\n').length);
    expect(reindexes).toBe(1);
  });

  it('generates a 6-character lowercase id on a list item line', async () => {
    const out = unwrap(await tool.cb({ name: 'Notes', line: 9 }));
    expect(out.id).toMatch(/^[a-z0-9]{6}$/);
    expect((await lines())[8]).toBe(`- item one ^${out.id}`);
  });

  it('returns an existing id without writing and flags an ignored request', async () => {
    const out = unwrap(await tool.cb({ name: 'Notes', line: 10, id: 'other' }));
    expect(out).toMatchObject({ id: 'two', created: false, line: 10, requestedIdIgnored: 'other' });
    const own = unwrap(await tool.cb({ name: 'Notes', line: 22 }));
    expect(own).toMatchObject({ id: 'callout', created: false, line: 24 });
    expect(own.requestedIdIgnored).toBeUndefined();
    expect((await lines()).join('\n')).toBe(NOTE);
    expect(reindexes).toBe(0);
  });

  it('puts a table id on its own line, set off by blank lines', async () => {
    const out = unwrap(await tool.cb({ name: 'Notes', line: 13, id: 'tbl' }));
    expect(out.line).toBe(16);
    expect((await lines()).slice(13, 18)).toEqual(['| 1 | 2 |', '', '^tbl', '', 'After table.']);
  });

  it('puts a code block id after the closing fence, from any fence line', async () => {
    const out = unwrap(await tool.cb({ name: 'Notes', line: 17, id: 'code' }));
    expect(out.line).toBe(21);
    expect((await lines()).slice(18, 22)).toEqual(['```', '', '^code', '']);
  });

  it('targets a heading line by heading text', async () => {
    const out = unwrap(await tool.cb({ name: 'Notes', heading: '# Intro', id: 'top' }));
    expect(out).toMatchObject({ line: 4, link: '[[Notes#^top]]' });
    expect((await lines())[3]).toBe('# Intro ^top');
    const again = unwrap(await tool.cb({ name: 'Notes', heading: 'Intro' }));
    expect(again).toMatchObject({ id: 'top', created: false });
  });

  it('dryRun returns the id and diff and leaves the file alone', async () => {
    const out = unwrap(await tool.cb({ name: 'Notes', line: 28, id: 'end', dryRun: true }));
    expect(out).toMatchObject({ id: 'end', created: true, dryRun: true, line: 28 });
    expect(out.diff).toContain('+Last line ^end');
    expect((await lines()).join('\n')).toBe(NOTE);
    expect(reindexes).toBe(0);
  });

  it('refuses bad targets and ids', async () => {
    const cases: Array<[Record<string, unknown>, RegExp]> = [
      [{}, /exactly one of `line` or `heading`/],
      [{ line: 4, heading: 'Intro' }, /exactly one/],
      [{ line: 6, id: 'bad id' }, /invalid/],
      [{ line: 6, id: 'two' }, /\^two is already used/],
      [{ line: 2 }, /frontmatter/],
      [{ line: 5 }, /blank/],
      [{ line: 99 }, /past the end/],
      [{ heading: 'Missing' }, /No heading "Missing"/],
      [{ heading: 'Twice' }, /appears 2 times .*lines 26, 27/],
    ];
    for (const [args, re] of cases) {
      const res = await tool.cb({ name: 'Notes', ...args });
      expect(res.isError, JSON.stringify(args)).toBe(true);
      expect(res.content[0].text).toMatch(re);
    }
    expect((await tool.cb({ name: 'Nope', line: 1 })).content[0].text).toMatch(/No note found/);
    expect((await lines()).join('\n')).toBe(NOTE);
  });

  it('links by path when another note shares the basename', async () => {
    await mkdir(join(vault, 'Archive'));
    await writeFile(join(vault, 'Archive', 'Notes.md'), 'Old text\n');
    upsertNode(db, { id: 'Archive/Notes.md', title: 'Old notes', content: '', frontmatter: {} });
    const out = unwrap(await tool.cb({ name: 'Archive/Notes.md', line: 1, id: 'old' }));
    expect(out.link).toBe('[[Archive/Notes#^old]]');
    expect(await lines('Archive/Notes.md')).toEqual(['Old text ^old', '']);
  });

  it('keeps CRLF endings on inline and own-line ids', async () => {
    await writeFile(join(vault, 'Notes.md'), NOTE.replace(/\n/g, '\r\n'));
    unwrap(await tool.cb({ name: 'Notes', line: 6, id: 'p' }));
    unwrap(await tool.cb({ name: 'Notes', line: 18, id: 'c' }));
    const text = await readFile(join(vault, 'Notes.md'), 'utf-8');
    expect(text).toContain('second line. ^p\r\n');
    expect(text).toContain('```\r\n\r\n^c\r\n\r\n');
    expect(text.replace(/\r\n/g, '')).not.toContain('\n');
  });

  it('adds an own-line id at the end of a file without a final newline', async () => {
    await writeFile(join(vault, 'Notes.md'), 'Intro\r\n\r\n> quote one\r\n> quote two');
    const out = unwrap(await tool.cb({ name: 'Notes', line: 3, id: 'q' }));
    expect(out.line).toBe(6);
    expect(await readFile(join(vault, 'Notes.md'), 'utf-8')).toBe('Intro\r\n\r\n> quote one\r\n> quote two\r\n\r\n^q');
  });
});
