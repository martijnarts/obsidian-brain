import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { join } from 'node:path';
import { z } from 'zod';
import { openDb, type DatabaseHandle } from '../../src/store/db.js';
import { Embedder } from '../../src/embeddings/embedder.js';
import { IndexPipeline } from '../../src/pipeline/indexer.js';
import { registerReindexTool } from '../../src/tools/reindex.js';
import type { ServerContext } from '../../src/context.js';

const FIXTURE_VAULT = join(import.meta.dirname, '..', 'fixtures', 'vault');

/**
 * Mock of `McpServer.tool()` that also replays the schema-based input
 * validation the real MCP SDK applies before dispatching to the handler.
 */
interface RecordedTool {
  name: string;
  description: string;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  cb: (args: any) => Promise<any>;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  schema: any;
}

function makeValidatingMockServer(): {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  server: any;
  registered: RecordedTool[];
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  invoke: (name: string, rawArgs: Record<string, unknown>) => Promise<any>;
} {
  const registered: RecordedTool[] = [];
  const server = {
    tool(
      name: string,
      description: string,
      schema: Record<string, z.ZodTypeAny>,
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      cb: (args: any) => Promise<any>,
    ): void {
      registered.push({ name, description, cb, schema });
    },
  };
  const invoke = async (
    name: string,
    rawArgs: Record<string, unknown>,
  ): Promise<unknown> => {
    const tool = registered.find((t) => t.name === name);
    if (!tool) throw new Error(`tool not registered: ${name}`);
    const parsed = z.object(tool.schema).parse(rawArgs);
    return tool.cb(parsed);
  };
  return { server, registered, invoke };
}

describe('tools/reindex', () => {
  let db: DatabaseHandle;
  let embedder: Embedder;
  let pipeline: IndexPipeline;

  beforeAll(async () => {
    db = openDb(':memory:');
    embedder = new Embedder();
    await embedder.init();
    pipeline = new IndexPipeline(db, embedder);
  }, 180_000);

  afterAll(async () => {
    db.close();
    await embedder.dispose();
  });

  const makeCtx = (): ServerContext =>
    ({
      db,
      pipeline,
      config: { vaultPath: FIXTURE_VAULT },
      embedderReady: () => true,
      ensureEmbedderReady: async () => undefined,
      lastManualReindexReason: null,
    }) as unknown as ServerContext;

  it('indexes a non-empty vault and records a manual reindex reason', async () => {
    const { server, invoke } = makeValidatingMockServer();
    const ctx = makeCtx();
    registerReindexTool(server, ctx);

    const result = await invoke('reindex', {});
    expect(result.isError).toBeFalsy();
    const payload = JSON.parse(result.content[0].text);
    expect(payload.nodesIndexed).toBeGreaterThan(0);
    expect(payload).toHaveProperty('stubsPruned');
    // v1.7.20 C8: tool call sets a manual reindex reason on the context
    // so index_status.lastReindexReasons isn't empty after a user-triggered
    // reindex.
    expect(ctx.lastManualReindexReason).toBe('user-triggered reindex');
  }, 180_000);

  it('a second reindex on an unchanged vault indexes nothing', async () => {
    const { server, invoke } = makeValidatingMockServer();
    registerReindexTool(server, makeCtx());

    const result = await invoke('reindex', {});
    expect(result.isError).toBeFalsy();
    const payload = JSON.parse(result.content[0].text);
    expect(payload.nodesIndexed).toBe(0);
  }, 60_000);
});
