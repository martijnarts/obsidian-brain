/**
 * `edit_note({ from_buffer: true })` retries the last failed replace_window
 * edit of a note. One process can serve several vaults, so the buffer must
 * never hand one vault's failed edit to a note of the same name in another.
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdtemp, writeFile, readFile, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { openDb, type DatabaseHandle } from '../../src/store/db.js';
import { upsertNode } from '../../src/store/nodes.js';
import { registerEditNoteTool } from '../../src/tools/edit-note.js';
import type { ServerContext } from '../../src/context.js';

interface Vault {
  path: string;
  db: DatabaseHandle;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  editNote: (args: any) => Promise<any>;
}

const NOTE = 'shared.md';
const ORIGINAL = '# Shared\n\nThe quick brown fox jumps over the lazy dog.\n';

async function makeVault(prefix: string): Promise<Vault> {
  const path = await mkdtemp(join(tmpdir(), prefix));
  const db = openDb(':memory:');
  await writeFile(join(path, NOTE), ORIGINAL, 'utf-8');
  upsertNode(db, { id: NOTE, title: 'Shared', content: '', frontmatter: {} });
  const ctx = {
    db,
    config: { vaultPath: path },
    ensureEmbedderReady: async () => {},
    pipeline: { index: async () => undefined },
    enqueueBackgroundReindex: () => {},
  } as unknown as ServerContext;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let editNote: ((args: any) => Promise<any>) | undefined;
  const server = {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    tool(name: string, _desc: string, _schema: unknown, cb: (args: any) => Promise<any>): void {
      if (name === 'edit_note') editNote = cb;
    },
  };
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  registerEditNoteTool(server as any, ctx);
  return { path, db, editNote: editNote! };
}

// A near-miss of the note's text: the exact match fails, the fuzzy retry
// from the buffer succeeds.
const failingEdit = {
  name: NOTE,
  mode: 'replace_window',
  search: 'The quick brown fax jumps over the lazy dog.',
  content: 'The slow red fox naps.',
};

describe('edit_note from_buffer across vaults', () => {
  let alpha: Vault;
  let beta: Vault;

  beforeEach(async () => {
    alpha = await makeVault('ob-buffer-alpha-');
    beta = await makeVault('ob-buffer-beta-');
  });

  afterEach(async () => {
    for (const vault of [alpha, beta]) {
      vault.db.close();
      await rm(vault.path, { recursive: true, force: true });
    }
  });

  it('retries a failed edit from the buffer in the same vault', async () => {
    const failed = await alpha.editNote(failingEdit);
    expect(failed.isError).toBe(true);

    const retried = await alpha.editNote({ name: NOTE, from_buffer: true });
    expect(retried.isError).toBeFalsy();
    expect(await readFile(join(alpha.path, NOTE), 'utf-8')).toContain('The slow red fox naps.');
  });

  it('does not offer a failed edit to the same note name in another vault', async () => {
    const failed = await alpha.editNote(failingEdit);
    expect(failed.isError).toBe(true);

    const crossVault = await beta.editNote({ name: NOTE, from_buffer: true });
    expect(crossVault.isError).toBe(true);
    expect(crossVault.content[0].text).toMatch(/No buffered edit found/);
    expect(await readFile(join(beta.path, NOTE), 'utf-8')).toBe(ORIGINAL);
  });
});
