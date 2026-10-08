/**
 * `edit_note({ expectedContent })` guards edits that replace text: the edit
 * runs only when the text it replaces still equals what the caller read.
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdtemp, writeFile, readFile, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { openDb, type DatabaseHandle } from '../../src/store/db.js';
import { upsertNode } from '../../src/store/nodes.js';
import { registerEditNoteTool } from '../../src/tools/edit-note.js';
import { applyEdit, checkExpectedContent } from '../../src/vault/editor.js';
import type { ServerContext } from '../../src/context.js';

const NOTE = 'note.md';
const ORIGINAL = '# Plan\n\nStep one.\nStep two.\n\n# Notes\n\nKeep this.\n';

describe('edit_note expectedContent', () => {
  let vault: string;
  let db: DatabaseHandle;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let editNote: (args: any) => Promise<any>;

  beforeEach(async () => {
    vault = await mkdtemp(join(tmpdir(), 'ob-expected-'));
    db = openDb(':memory:');
    await writeFile(join(vault, NOTE), ORIGINAL, 'utf-8');
    upsertNode(db, { id: NOTE, title: 'Note', content: '', frontmatter: {} });
    const ctx = {
      db,
      config: { vaultPath: vault },
      ensureEmbedderReady: async () => {},
      pipeline: { index: async () => undefined },
      enqueueBackgroundReindex: () => {},
    } as unknown as ServerContext;
    const server = {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      tool(name: string, _d: string, _s: unknown, cb: (args: any) => Promise<any>): void {
        if (name === 'edit_note') editNote = cb;
      },
    };
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    registerEditNoteTool(server as any, ctx);
  });

  afterEach(async () => {
    db.close();
    await rm(vault, { recursive: true, force: true });
  });

  const read = () => readFile(join(vault, NOTE), 'utf-8');
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const errorText = (result: any): string => {
    expect(result.isError).toBe(true);
    return result.content[0].text as string;
  };

  it('applies a replace_window edit whose replaced text still matches', async () => {
    const result = await editNote({
      name: NOTE,
      mode: 'replace_window',
      search: 'Step two.',
      content: 'Step 2.',
      expectedContent: 'Step two.',
    });
    expect(result.isError).toBeFalsy();
    expect(await read()).toContain('Step 2.');
  });

  it('applies a patch_heading replace when the section still matches', async () => {
    const result = await editNote({
      name: NOTE,
      mode: 'patch_heading',
      heading: 'Plan',
      headingOp: 'replace',
      content: '\nOnly step.\n\n',
      expectedContent: '\nStep one.\nStep two.\n\n',
    });
    expect(result.isError).toBeFalsy();
    expect(await read()).toContain('Only step.');
    expect(await read()).toContain('Keep this.');
  });

  it('refuses a patch_heading replace when someone changed the section', async () => {
    const result = await editNote({
      name: NOTE,
      mode: 'patch_heading',
      heading: 'Plan',
      headingOp: 'replace',
      content: '\nOnly step.\n\n',
      expectedContent: 'Step one.\nStep three.',
    });
    expect(errorText(result)).toMatch(/no longer matches expectedContent/);
    expect(errorText(result)).toContain('Step two.');
    expect(await read()).toBe(ORIGINAL);
  });

  it('applies an at_line replace when the line still matches', async () => {
    const result = await editNote({
      name: NOTE,
      mode: 'at_line',
      line: 3,
      lineOp: 'replace',
      content: 'First step.',
      expectedContent: 'Step one.',
    });
    expect(result.isError).toBeFalsy();
    expect((await read()).split('\n')[2]).toBe('First step.');
  });

  it('refuses an at_line replace on a line that moved', async () => {
    const result = await editNote({
      name: NOTE,
      mode: 'at_line',
      line: 4,
      lineOp: 'replace',
      content: 'First step.',
      expectedContent: 'Step one.',
    });
    expect(errorText(result)).toMatch(/no longer matches/);
    expect(await read()).toBe(ORIGINAL);
  });

  it('ignores line endings and trailing whitespace in the comparison', async () => {
    const result = await editNote({
      name: NOTE,
      mode: 'patch_heading',
      heading: 'Plan',
      headingOp: 'replace',
      content: '\nOnly step.\n\n',
      expectedContent: 'Step one.  \r\nStep two.\r\n',
    });
    expect(result.isError).toBeFalsy();
  });

  it('fails a dry run with stale text, so no preview is made', async () => {
    const result = await editNote({
      name: NOTE,
      mode: 'at_line',
      line: 3,
      lineOp: 'replace',
      content: 'X',
      expectedContent: 'something else',
      dryRun: true,
    });
    expect(errorText(result)).toMatch(/no longer matches/);
  });

  it('passes a dry run with matching text', async () => {
    const result = await editNote({
      name: NOTE,
      mode: 'at_line',
      line: 3,
      lineOp: 'replace',
      content: 'X',
      expectedContent: 'Step one.',
      dryRun: true,
    });
    expect(JSON.parse(result.content[0].text)).toMatchObject({ dryRun: true });
    expect(await read()).toBe(ORIGINAL);
  });

  it('refuses expectedContent on an edit that replaces nothing', async () => {
    const result = await editNote({
      name: NOTE,
      mode: 'append',
      content: '\nMore.',
      expectedContent: 'anything',
    });
    expect(errorText(result)).toMatch(/applies only to edits that replace text/);
    expect(await read()).toBe(ORIGINAL);
  });

  it('refuses expectedContent with bulk edits', async () => {
    const result = await editNote({
      name: NOTE,
      edits: [{ mode: 'append', content: 'x' }],
      expectedContent: 'Step one.',
    });
    expect(errorText(result)).toMatch(/single edit, not with `edits`/);
  });
});

describe('checkExpectedContent', () => {
  it('compares against exactly the replaced range', () => {
    const original = 'aaa\nbbb\nccc\n';
    const res = applyEdit(original, { kind: 'replace_window', search: 'bbb', content: 'BBB' });
    expect(() => checkExpectedContent(original, res, 'bbb')).not.toThrow();
    expect(() => checkExpectedContent(original, res, 'aaa')).toThrow(/no longer matches/);
  });
});
