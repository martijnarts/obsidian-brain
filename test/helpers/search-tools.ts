/**
 * Fixtures shared by the find_notes_by_name, grep_vault and query_notes
 * tests: a mock server that also keeps each tool's Zod shape (so tests can
 * check argument validation), a temp-vault writer, and an indexer that
 * fills the DB from the files on disk the way a reindex does.
 */
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { expect } from 'vitest';
import { z } from 'zod';
import type { DatabaseHandle } from '../../src/store/db.js';
import { upsertNode } from '../../src/store/nodes.js';
import { parseVault } from '../../src/vault/parser.js';
import type { ServerContext } from '../../src/context.js';

export { unwrap } from './mock-server.js';

export interface SchemaTool {
  name: string;
  schema: z.ZodRawShape;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  cb: (args: any) => Promise<any>;
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function makeSchemaServer(): { server: any; registered: SchemaTool[] } {
  const registered: SchemaTool[] = [];
  const server = {
    tool(
      name: string,
      _d: string,
      schema: z.ZodRawShape,
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      cb: (args: any) => Promise<any>,
    ): void {
      registered.push({ name, schema, cb });
    },
  };
  return { server, registered };
}

/** The error text of an `isError: true` result; fails the test otherwise. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function errorText(result: any): string {
  expect(result.isError).toBe(true);
  return result.content[0].text as string;
}

/** True when `args` passes the tool's own Zod shape. */
export function acceptsArgs(tool: SchemaTool, args: unknown): boolean {
  return z.object(tool.schema).safeParse(args).success;
}

export function buildCtx(db: DatabaseHandle, vaultPath: string): ServerContext {
  return { db, config: { vaultPath } } as unknown as ServerContext;
}

/** Write `files` (vault-relative path → content) into `vault`. */
export async function writeVault(vault: string, files: Record<string, string>): Promise<void> {
  for (const [rel, content] of Object.entries(files)) {
    await mkdir(dirname(join(vault, rel)), { recursive: true });
    await writeFile(join(vault, rel), content);
  }
}

/** Index the vault on disk into `db`: one node per note plus a stub per unresolved link. */
export async function indexVault(db: DatabaseHandle, vault: string): Promise<void> {
  const { nodes, stubIds } = await parseVault(vault);
  for (const node of nodes) upsertNode(db, node);
  for (const id of stubIds) {
    upsertNode(db, { id, title: id.slice('_stub/'.length, -3), content: '', frontmatter: { _stub: true } });
  }
}
