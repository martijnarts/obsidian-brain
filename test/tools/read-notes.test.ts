import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { upsertNode } from '../../src/store/nodes.js';
import { registerReadNotesTool } from '../../src/tools/read-notes.js';
import { makeHarness, loadTool, ok, err, type FileToolHarness } from '../helpers/file-tools.js';

describe('tools/read_notes', () => {
  let h: FileToolHarness;
  let tool: ReturnType<typeof loadTool>;

  beforeEach(async () => {
    h = await makeHarness();
    upsertNode(h.db, { id: 'Alpha.md', title: 'Alpha', content: 'alpha body', frontmatter: { tags: ['a'] } });
    upsertNode(h.db, { id: 'dir/Beta.md', title: 'Beta', content: 'b'.repeat(3000), frontmatter: {} });
    upsertNode(h.db, { id: 'x/Gamma.md', title: 'Gamma one', content: '', frontmatter: {} });
    upsertNode(h.db, { id: 'y/Gamma.md', title: 'Gamma two', content: '', frontmatter: {} });
    tool = loadTool(registerReadNotesTool, h.ctx);
  });

  afterEach(async () => {
    await h.dispose();
  });

  it('reads several notes in order, by title and by path', async () => {
    const { notes } = ok(await tool.call({ names: ['Alpha', 'dir/Beta.md'] }));
    expect(notes).toHaveLength(2);
    expect(notes[0]).toEqual({
      path: 'Alpha.md',
      title: 'Alpha',
      frontmatter: { tags: ['a'] },
      content: 'alpha body',
      truncated: false,
    });
    expect(notes[1].path).toBe('dir/Beta.md');
  });

  it('truncates each body at 2000 chars by default', async () => {
    const { notes } = ok(await tool.call({ names: ['Beta'] }));
    expect(notes[0].content).toHaveLength(2000);
    expect(notes[0].truncated).toBe(true);
  });

  it('honours maxContentLength per note', async () => {
    const { notes } = ok(await tool.call({ names: ['Alpha', 'Beta'], maxContentLength: 5 }));
    expect(notes[0].content).toBe('alpha');
    expect(notes[0].truncated).toBe(true);
    expect(notes[1].content).toBe('bbbbb');
  });

  it('a missing or ambiguous name fails only its own entry', async () => {
    const { notes } = ok(await tool.call({ names: ['Nope', 'Alpha', 'gamma'] }));
    expect(notes[0]).toEqual({ name: 'Nope', error: 'No note found matching "Nope"' });
    expect(notes[1].path).toBe('Alpha.md');
    expect(notes[2].name).toBe('gamma');
    expect(notes[2].error).toMatch(/Multiple notes match "gamma"/);
  });

  it('rejects an empty list and more than 20 names', async () => {
    expect(err(await tool.call({ names: [] }))).toMatch(/invalid arguments/);
    const many = Array.from({ length: 21 }, () => 'Alpha');
    expect(err(await tool.call({ names: many }))).toMatch(/invalid arguments/);
    expect(ok(await tool.call({ names: many.slice(0, 20) })).notes).toHaveLength(20);
  });
});
