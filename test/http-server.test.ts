/**
 * In-process tests for `startHttpServer`. The subprocess suite in
 * test/integration/multi-vault.test.ts covers the CLI and signals; this one
 * runs the server in the test process so coverage can see it.
 *
 * The embedder points at an unreachable Ollama, so no model loads and the
 * background startup index fails fast.
 */

import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { startHttpServer, type HttpServerHandle } from '../src/http-server.js';
import { EXPOSED_TOOL_COUNT } from './helpers/tool-count.js';

async function connect(port: number, path = '/mcp'): Promise<Client> {
  const client = new Client({ name: 'http-server-test', version: '0.0.0' });
  await client.connect(new StreamableHTTPClientTransport(new URL(`http://127.0.0.1:${port}${path}`)));
  return client;
}

describe('startHttpServer', () => {
  let alpha: string;
  let beta: string;
  let dataDir: string;
  let handle: HttpServerHandle | null;

  beforeEach(() => {
    vi.stubEnv('EMBEDDING_PROVIDER', 'ollama');
    vi.stubEnv('OLLAMA_BASE_URL', 'http://127.0.0.1:1');
    vi.stubEnv('OLLAMA_EMBEDDING_DIM', '384');
    vi.stubEnv('OBSIDIAN_BRAIN_NO_WATCH', '1');
    alpha = mkdtempSync(join(tmpdir(), 'ob-http-alpha-'));
    beta = mkdtempSync(join(tmpdir(), 'ob-http-beta-'));
    dataDir = mkdtempSync(join(tmpdir(), 'ob-http-data-'));
    writeFileSync(join(alpha, 'a.md'), '# A\n');
    writeFileSync(join(beta, 'b.md'), '# B\n');
    handle = null;
  });

  afterEach(async () => {
    await handle?.close();
    vi.unstubAllEnvs();
    for (const dir of [alpha, beta, dataDir]) rmSync(dir, { recursive: true, force: true });
  });

  function start(): Promise<HttpServerHandle> {
    return startHttpServer({
      host: '127.0.0.1',
      port: 0,
      vaults: [
        { name: 'alpha', vaultPath: alpha },
        { name: 'beta', vaultPath: beta },
      ],
      dataDir,
    });
  }

  it('binds a free port when asked for port 0 and opens one index per vault', async () => {
    handle = await start();
    expect(handle.port).toBeGreaterThan(0);
    expect(existsSync(join(dataDir, 'alpha', 'kg.db'))).toBe(true);
    expect(existsSync(join(dataDir, 'beta', 'kg.db'))).toBe(true);
  });

  it('sends each call to the vault it names', async () => {
    handle = await start();
    const client = await connect(handle.port);
    expect((await client.listTools()).tools).toHaveLength(EXPOSED_TOOL_COUNT);
    for (const [name, dir, other] of [
      ['alpha', alpha, beta],
      ['beta', beta, alpha],
    ] as const) {
      const result = await client.callTool({
        name: 'create_note',
        arguments: { vault: name, title: `From ${name}`, content: name },
      });
      expect(result.isError).toBeFalsy();
      expect(readFileSync(join(dir, `From ${name}.md`), 'utf-8')).toContain(name);
      expect(existsSync(join(other, `From ${name}.md`))).toBe(false);
    }
    await client.close();
  });

  it('accepts a trailing slash on /mcp', async () => {
    handle = await start();
    const client = await connect(handle.port, '/mcp/');
    expect((await client.listTools()).tools).toHaveLength(EXPOSED_TOOL_COUNT);
    await client.close();
  });

  it('answers 404 outside /mcp and 405 for non-POST', async () => {
    handle = await start();
    const base = `http://127.0.0.1:${handle.port}`;
    expect((await fetch(`${base}/alpha/mcp`, { method: 'POST' })).status).toBe(404);
    expect((await fetch(`${base}/`, { method: 'POST' })).status).toBe(404);
    const get = await fetch(`${base}/mcp`);
    expect(get.status).toBe(405);
    expect(get.headers.get('allow')).toBe('POST');
    expect(await get.json()).toMatchObject({ jsonrpc: '2.0', error: { code: -32000 } });
  });

  it('runs the startup index for every vault in the background', async () => {
    handle = await start();
    // Each vault starts with an empty index. The startup index fills both,
    // one after the other; the unreachable embedder only leaves the notes
    // without embeddings.
    await vi.waitFor(
      async () => {
        const client = await connect(handle!.port);
        const result = await client.callTool({ name: 'list_vaults', arguments: {} });
        await client.close();
        const vaults = JSON.parse((result.content as Array<{ text: string }>)[0]!.text) as Array<{
          name: string;
          notes: number;
        }>;
        expect(vaults.map((v) => [v.name, v.notes])).toEqual([
          ['alpha', 1],
          ['beta', 1],
        ]);
      },
      { timeout: 10_000, interval: 200 },
    );
  });

  it('starts and closes a watcher per vault unless OBSIDIAN_BRAIN_NO_WATCH=1', async () => {
    vi.stubEnv('OBSIDIAN_BRAIN_NO_WATCH', '0');
    handle = await start();
    await handle.close();
    handle = null;
  });

  it('closes once, however often close is called', async () => {
    handle = await start();
    const first = handle.close();
    expect(handle.close()).toBe(first);
    await first;
    await expect(fetch(`http://127.0.0.1:${handle.port}/mcp`)).rejects.toThrow();
    handle = null;
  });

  it('rejects when the port is taken, after closing the vaults it opened', async () => {
    const blocker = createServer();
    await new Promise<void>((resolve) => blocker.listen(0, '127.0.0.1', () => resolve()));
    const port = (blocker.address() as { port: number }).port;
    try {
      await expect(
        startHttpServer({
          host: '127.0.0.1',
          port,
          vaults: [{ name: 'alpha', vaultPath: alpha }],
          dataDir,
        }),
      ).rejects.toThrow(/EADDRINUSE/);
    } finally {
      blocker.close();
    }
  });
});
