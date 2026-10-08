/**
 * Harness for the file tools: a temp vault on disk, an in-memory index, and
 * a caller that validates arguments against the tool's Zod shape before it
 * runs the handler, as the MCP SDK does.
 */

import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';
import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { openDb, type DatabaseHandle } from '../../src/store/db.js';
import type { ServerContext } from '../../src/context.js';

export interface FileToolHarness {
  db: DatabaseHandle;
  vault: string;
  ctx: ServerContext;
  /** How many background reindexes the tools queued. */
  reindexes: () => number;
  write: (rel: string, content: string | Buffer) => Promise<void>;
  dispose: () => Promise<void>;
}

export async function makeHarness(): Promise<FileToolHarness> {
  const db = openDb(':memory:');
  const vault = await mkdtemp(join(tmpdir(), 'kg-files-'));
  let reindexes = 0;
  const ctx = {
    db,
    config: { vaultPath: vault },
    ensureEmbedderReady: async () => {},
    pipeline: {
      index: async () => {
        reindexes++;
      },
    },
  } as unknown as ServerContext;
  return {
    db,
    vault,
    ctx,
    reindexes: () => reindexes,
    write: async (rel, content) => {
      await mkdir(dirname(join(vault, rel)), { recursive: true });
      await writeFile(join(vault, rel), content);
    },
    dispose: async () => {
      // Let any fire-and-forget reindex settle before the vault goes.
      await new Promise((r) => setTimeout(r, 0));
      db.close();
      await rm(vault, { recursive: true, force: true });
    },
  };
}

/** A tool call result: `ok` with the parsed payload, or the error text. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type CallResult = { ok: true; data: any } | { ok: false; error: string };

export function loadTool(
  register: (server: McpServer, ctx: ServerContext) => void,
  ctx: ServerContext,
): { call: (args: Record<string, unknown>) => Promise<CallResult>; schema: z.ZodObject<z.ZodRawShape> } {
  let shape: z.ZodRawShape = {};
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let cb: (args: any) => Promise<any> = async () => undefined;
  const server = {
    tool(_n: string, _d: string, s: z.ZodRawShape, c: typeof cb) {
      shape = s;
      cb = c;
    },
  };
  register(server as unknown as McpServer, ctx);
  const schema = z.object(shape);
  return {
    schema,
    call: async (args) => {
      const parsed = schema.safeParse(args);
      if (!parsed.success) return { ok: false, error: `invalid arguments: ${parsed.error.message}` };
      const result = await cb(parsed.data);
      const text = result.content[0].text as string;
      if (result.isError) return { ok: false, error: text };
      return { ok: true, data: JSON.parse(text) };
    },
  };
}

/** Unwrap an `ok` result or fail the test with the tool's error. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function ok(result: CallResult): any {
  if (!result.ok) throw new Error(`expected success, got: ${result.error}`);
  return result.data;
}

/** Unwrap an error result or fail the test. */
export function err(result: CallResult): string {
  if (result.ok) throw new Error(`expected an error, got: ${JSON.stringify(result.data)}`);
  return result.error;
}
