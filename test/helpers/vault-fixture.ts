/**
 * Temp vault + in-memory index for the maintenance-tool tests. Files are
 * written to disk, parsed with the real `parseVault`, and loaded into an
 * `openDb(':memory:')` store with stub nodes, as the indexer does.
 * `ctx.reindexCalls` counts background reindexes.
 */

import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';
import { openDb, type DatabaseHandle } from '../../src/store/db.js';
import { upsertNode } from '../../src/store/nodes.js';
import { insertEdge } from '../../src/store/edges.js';
import { parseVault } from '../../src/vault/parser.js';
import { materialiseStubs } from '../../src/pipeline/indexer/stubs.js';
import type { ServerContext } from '../../src/context.js';

export interface VaultFixture {
  vault: string;
  db: DatabaseHandle;
  ctx: ServerContext & { reindexCalls: number };
  read(rel: string): Promise<string>;
  cleanup(): Promise<void>;
}

export async function makeVault(files: Record<string, string>): Promise<VaultFixture> {
  const vault = await mkdtemp(join(tmpdir(), 'kg-maint-'));
  for (const [rel, content] of Object.entries(files)) {
    await mkdir(dirname(join(vault, rel)), { recursive: true });
    await writeFile(join(vault, rel), content, 'utf-8');
  }
  const db = openDb(':memory:');
  const parsed = await parseVault(vault);
  for (const node of parsed.nodes) upsertNode(db, node);
  for (const edge of parsed.edges) insertEdge(db, edge);
  materialiseStubs(db, parsed.stubIds);

  const ctx = {
    db,
    config: { vaultPath: vault },
    reindexCalls: 0,
    ensureEmbedderReady: async () => {},
    pipeline: { index: async () => undefined },
    enqueueBackgroundReindex: () => {
      ctx.reindexCalls++;
    },
  } as unknown as ServerContext & { reindexCalls: number };

  return {
    vault,
    db,
    ctx,
    read: (rel) => readFile(join(vault, rel), 'utf-8'),
    cleanup: async () => {
      db.close();
      await rm(vault, { recursive: true, force: true });
    },
  };
}
