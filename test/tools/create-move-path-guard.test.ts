/**
 * `create_note` and `move_note` take a directory, title or destination from
 * the client. None of them may put a file outside the vault: not with `..`,
 * not with an absolute path, and not through a symlink that points outside.
 */

import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { openDb, type DatabaseHandle } from '../../src/store/db.js';
import { upsertNode } from '../../src/store/nodes.js';
import { VaultWriter } from '../../src/vault/writer.js';
import { registerCreateNoteTool } from '../../src/tools/create-note.js';
import { registerMoveNoteTool } from '../../src/tools/move-note.js';
import type { ServerContext } from '../../src/context.js';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Cb = (args: any) => Promise<any>;

describe('create_note and move_note stay inside the vault', () => {
  let root: string;
  let vault: string;
  let outside: string;
  let db: DatabaseHandle;
  const tools: Record<string, Cb> = {};

  beforeEach(() => {
    root = mkdtempSync(join(tmpdir(), 'ob-guard-'));
    vault = join(root, 'vault');
    outside = join(root, 'outside');
    mkdirSync(vault);
    mkdirSync(outside);
    writeFileSync(join(vault, 'A.md'), '# A\n');
    // A folder inside the vault that is really a link to a folder outside it.
    symlinkSync(outside, join(vault, 'linked'));
    db = openDb(':memory:');
    upsertNode(db, { id: 'A.md', title: 'A', content: '', frontmatter: {} });
    const ctx = {
      db,
      config: { vaultPath: vault },
      writer: new VaultWriter(vault, db),
      ensureEmbedderReady: async () => {},
      pipeline: { index: async () => undefined },
      enqueueBackgroundReindex: () => {},
    } as unknown as ServerContext;
    const server = {
      tool: (name: string, _d: string, _s: unknown, cb: Cb) => {
        tools[name] = cb;
      },
    };
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    registerCreateNoteTool(server as any, ctx);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    registerMoveNoteTool(server as any, ctx);
  });

  afterEach(() => {
    db.close();
    rmSync(root, { recursive: true, force: true });
  });

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const errorText = (result: any): string => {
    expect(result.isError).toBe(true);
    return result.content[0].text as string;
  };
  const nothingOutside = () => {
    expect(readdirSync(outside)).toEqual([]);
    expect(readdirSync(root).sort()).toEqual(['outside', 'vault']);
  };

  it.each([
    ['a `..` directory', { title: 'X', directory: '../../escaped' }],
    ['a `..` title', { title: '../escaped' }],
    ['a `..` inside the directory', { title: 'X', directory: 'notes/../../escaped' }],
    ['an absolute directory', { title: 'X', directory: '/tmp/ob-guard-escape' }],
    ['a symlinked directory', { title: 'X', directory: 'linked' }],
  ])('create_note refuses %s', async (_label, args) => {
    const result = await tools.create_note!({ content: 'body', ...args });
    expect(errorText(result)).toMatch(/outside the vault/);
    nothingOutside();
    expect(existsSync('/tmp/ob-guard-escape')).toBe(false);
  });

  it('create_note still creates a note in a nested folder', async () => {
    const result = await tools.create_note!({ title: 'Inside', content: 'body', directory: 'a/b' });
    expect(result.isError).toBeFalsy();
    expect(JSON.parse(result.content[0].text).path).toBe('a/b/Inside.md');
    expect(existsSync(join(vault, 'a', 'b', 'Inside.md'))).toBe(true);
  });

  it.each([
    ['a `..` destination', '../escaped.md'],
    ['a deep `..` destination', 'notes/../../../escaped.md'],
    ['an absolute destination', '/tmp/ob-guard-escape.md'],
    ['a symlinked destination folder', 'linked/A.md'],
  ])('move_note refuses %s', async (_label, destination) => {
    const result = await tools.move_note!({ source: 'A.md', destination });
    expect(errorText(result)).toMatch(/outside the vault/);
    expect(existsSync(join(vault, 'A.md'))).toBe(true);
    nothingOutside();
    expect(existsSync('/tmp/ob-guard-escape.md')).toBe(false);
  });

  it('move_note still moves a note into a new folder', async () => {
    const result = await tools.move_note!({ source: 'A.md', destination: 'archive/A.md' });
    expect(result.isError).toBeFalsy();
    expect(existsSync(join(vault, 'archive', 'A.md'))).toBe(true);
    expect(existsSync(join(vault, 'A.md'))).toBe(false);
  });
});
