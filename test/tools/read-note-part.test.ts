import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { upsertNode } from '../../src/store/nodes.js';
import { registerReadNotePartTool } from '../../src/tools/read-note-part.js';
import { makeHarness, loadTool, ok, err, type FileToolHarness } from '../helpers/file-tools.js';

const NOTE = [
  '---', //                     1
  'title: Plan', //             2
  '# not a heading', //         3
  '---', //                     4
  '# Plan', //                  5
  '', //                        6
  'Intro paragraph', //         7
  'second line ^intro', //      8
  '', //                        9
  '## Goals', //               10
  '- ship it ^goal1', //       11
  '- test it', //              12
  '', //                       13
  '```md', //                  14
  '# Fake heading', //         15
  'code ^fake', //             16
  '```', //                    17
  '', //                       18
  '### Detail', //             19
  'detail text', //            20
  '', //                       21
  '## Notes ##', //            22
  '| a | b |', //              23
  '| - | - |', //              24
  '', //                       25
  '^table', //                 26
  '', //                       27
  '# Archive', //              28
  '## Goals', //               29
  'old goals', //              30
  '',
].join('\n');

describe('tools/read_note_part', () => {
  let h: FileToolHarness;
  let tool: ReturnType<typeof loadTool>;

  beforeEach(async () => {
    h = await makeHarness();
    await h.write('Plan.md', NOTE);
    upsertNode(h.db, { id: 'Plan.md', title: 'Plan', content: '', frontmatter: {} });
    tool = loadTool(registerReadNotePartTool, h.ctx);
  });

  afterEach(async () => {
    await h.dispose();
  });

  describe('outline', () => {
    it('lists headings with level and 1-based line, skipping frontmatter and code fences', async () => {
      const data = ok(await tool.call({ name: 'Plan', mode: 'outline' }));
      expect(data.path).toBe('Plan.md');
      expect(data.headings).toEqual([
        { level: 1, text: 'Plan', line: 5 },
        { level: 2, text: 'Goals', line: 10 },
        { level: 3, text: 'Detail', line: 19 },
        { level: 2, text: 'Notes', line: 22 },
        { level: 1, text: 'Archive', line: 28 },
        { level: 2, text: 'Goals', line: 29 },
      ]);
    });

    it('returns an empty outline for a note without headings', async () => {
      await h.write('Plain.md', 'just text\n#tag is not a heading\n');
      upsertNode(h.db, { id: 'Plain.md', title: 'Plain', content: '', frontmatter: {} });
      expect(ok(await tool.call({ name: 'Plain', mode: 'outline' })).headings).toEqual([]);
    });
  });

  describe('heading', () => {
    it('returns the section down to the next heading of the same or higher level', async () => {
      const data = ok(await tool.call({ name: 'Plan', mode: 'heading', heading: 'Notes' }));
      expect(data.heading).toEqual({ level: 2, text: 'Notes', line: 22 });
      expect(data.startLine).toBe(22);
      expect(data.endLine).toBe(27);
      expect(data.content.split('\n')[0]).toBe('## Notes ##');
    });

    it('includes deeper headings in the section', async () => {
      const data = ok(await tool.call({ name: 'Plan', mode: 'heading', heading: 'Plan' }));
      expect(data.startLine).toBe(5);
      expect(data.endLine).toBe(27);
      expect(data.content).toContain('### Detail');
    });

    it('matches case-insensitively', async () => {
      const data = ok(await tool.call({ name: 'Plan', mode: 'heading', heading: 'detail' }));
      expect(data.content).toBe('### Detail\ndetail text\n');
    });

    it('runs to the end of the note when no closing heading follows', async () => {
      const data = ok(await tool.call({ name: 'Plan', mode: 'heading', heading: 'Archive' }));
      expect(data.endLine).toBe(30);
      expect(data.content).toBe('# Archive\n## Goals\nold goals');
    });

    it('refuses an ambiguous heading and names the lines', async () => {
      expect(err(await tool.call({ name: 'Plan', mode: 'heading', heading: 'Goals' }))).toMatch(
        /ambiguous.*line 10, line 29/,
      );
    });

    it('resolves a nested path', async () => {
      const data = ok(await tool.call({ name: 'Plan', mode: 'heading', heading: 'Archive > Goals' }));
      expect(data.startLine).toBe(29);
      expect(data.content).toBe('## Goals\nold goals');
      const deep = ok(await tool.call({ name: 'Plan', mode: 'heading', heading: 'plan > goals > detail' }));
      expect(deep.startLine).toBe(19);
    });

    it('names the missing segment of a nested path and its parent', async () => {
      expect(err(await tool.call({ name: 'Plan', mode: 'heading', heading: 'Archive > Detail' }))).toMatch(
        /Heading "Detail" not found under "Archive"/,
      );
      expect(err(await tool.call({ name: 'Plan', mode: 'heading', heading: 'Nope > Goals' }))).toMatch(
        /Heading "Nope" not found under the note/,
      );
    });

    it('a heading text that holds ">" matches as a whole first', async () => {
      await h.write('Arrow.md', '# A > B\nbody\n');
      upsertNode(h.db, { id: 'Arrow.md', title: 'Arrow', content: '', frontmatter: {} });
      const data = ok(await tool.call({ name: 'Arrow', mode: 'heading', heading: 'A > B' }));
      expect(data.content).toBe('# A > B\nbody');
    });

    it('errors on a missing heading, a heading inside code, and a missing argument', async () => {
      expect(err(await tool.call({ name: 'Plan', mode: 'heading', heading: 'Missing' }))).toMatch(
        /Heading "Missing" not found/,
      );
      expect(err(await tool.call({ name: 'Plan', mode: 'heading', heading: 'Fake heading' }))).toMatch(/not found/);
      expect(err(await tool.call({ name: 'Plan', mode: 'heading' }))).toMatch(/needs `heading`/);
    });
  });

  describe('lines', () => {
    it('returns an inclusive 1-based range that counts the frontmatter', async () => {
      const data = ok(await tool.call({ name: 'Plan', mode: 'lines', startLine: 1, endLine: 2 }));
      expect(data).toMatchObject({ startLine: 1, endLine: 2, totalLines: 30, content: '---\ntitle: Plan' });
    });

    it('defaults endLine to, and clamps it at, the end of the note', async () => {
      expect(ok(await tool.call({ name: 'Plan', mode: 'lines', startLine: 30 })).content).toBe('old goals');
      expect(ok(await tool.call({ name: 'Plan', mode: 'lines', startLine: 30, endLine: 999 })).endLine).toBe(30);
    });

    it('errors on a start past the end, an end before the start, and a missing start', async () => {
      expect(err(await tool.call({ name: 'Plan', mode: 'lines', startLine: 40 }))).toMatch(/past the end/);
      expect(err(await tool.call({ name: 'Plan', mode: 'lines', startLine: 5, endLine: 4 }))).toMatch(/before startLine/);
      expect(err(await tool.call({ name: 'Plan', mode: 'lines' }))).toMatch(/needs `startLine`/);
      expect(err(await tool.call({ name: 'Plan', mode: 'lines', startLine: 0 }))).toMatch(/invalid arguments/);
    });
  });

  describe('block', () => {
    it('returns the paragraph a block id ends', async () => {
      const data = ok(await tool.call({ name: 'Plan', mode: 'block', blockId: 'intro' }));
      expect(data).toMatchObject({ blockId: 'intro', startLine: 7, endLine: 8, content: 'Intro paragraph\nsecond line ^intro' });
    });

    it('returns only the list item for a block id on a list item, with or without ^', async () => {
      const data = ok(await tool.call({ name: 'Plan', mode: 'block', blockId: '^goal1' }));
      expect(data).toMatchObject({ startLine: 11, endLine: 11, content: '- ship it ^goal1' });
    });

    it('a block id on its own line marks the block above it', async () => {
      const data = ok(await tool.call({ name: 'Plan', mode: 'block', blockId: 'table' }));
      expect(data).toMatchObject({ startLine: 23, endLine: 26 });
      expect(data.content).toBe('| a | b |\n| - | - |\n\n^table');
    });

    it('ignores block ids inside code and errors on missing or empty ids', async () => {
      expect(err(await tool.call({ name: 'Plan', mode: 'block', blockId: 'fake' }))).toMatch(/Block "\^fake" not found/);
      expect(err(await tool.call({ name: 'Plan', mode: 'block', blockId: '^^' }))).toMatch(/blockId is empty/);
      expect(err(await tool.call({ name: 'Plan', mode: 'block' }))).toMatch(/needs `blockId`/);
    });
  });

  it('reads CRLF notes with the same line numbers', async () => {
    await h.write('Crlf.md', '# One\r\ntext ^b1\r\n# Two\r\n');
    upsertNode(h.db, { id: 'Crlf.md', title: 'Crlf', content: '', frontmatter: {} });
    expect(ok(await tool.call({ name: 'Crlf', mode: 'outline' })).headings).toEqual([
      { level: 1, text: 'One', line: 1 },
      { level: 1, text: 'Two', line: 3 },
    ]);
    expect(ok(await tool.call({ name: 'Crlf', mode: 'block', blockId: 'b1' })).content).toBe('text ^b1');
  });

  it('errors on a missing note, an ambiguous name and a bad mode', async () => {
    upsertNode(h.db, { id: 'a/Twin.md', title: 'Twin A', content: '', frontmatter: {} });
    upsertNode(h.db, { id: 'b/Twin.md', title: 'Twin B', content: '', frontmatter: {} });
    expect(err(await tool.call({ name: 'Nope', mode: 'outline' }))).toMatch(/No note found/);
    expect(err(await tool.call({ name: 'twin', mode: 'outline' }))).toMatch(/Multiple notes match/);
    expect(err(await tool.call({ name: 'Plan', mode: 'everything' }))).toMatch(/invalid arguments/);
  });

  it('errors when the indexed note is gone from disk', async () => {
    upsertNode(h.db, { id: 'Ghost.md', title: 'Ghost', content: '', frontmatter: {} });
    expect(err(await tool.call({ name: 'Ghost', mode: 'outline' }))).toMatch(/ENOENT/);
  });
});
