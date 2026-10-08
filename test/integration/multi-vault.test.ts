/**
 * Integration tests for `obsidian-brain server` with several vaults. Every
 * tool takes a required `vault` argument. The same suite runs over stdio
 * and over streamable HTTP.
 *
 * The tests spawn the built CLI with two temp vaults. The embedder points at
 * an unreachable Ollama, so no model loads; the tests cover routing, vault
 * isolation, argument errors and shutdown.
 */

import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { once } from 'node:events';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { EXPOSED_TOOL_COUNT } from '../helpers/tool-count.js';

const cliPath = join(process.cwd(), 'dist', 'cli', 'index.js');

function testEnv(dataDir: string): Record<string, string> {
  return {
    ...(process.env as Record<string, string>),
    DATA_DIR: dataDir,
    OBSIDIAN_BRAIN_NO_WATCH: '1',
    OBSIDIAN_BRAIN_NO_CATCHUP: '1',
    EMBEDDING_PROVIDER: 'ollama',
    OLLAMA_BASE_URL: 'http://127.0.0.1:1', // unreachable; background init fails fast
    OLLAMA_EMBEDDING_DIM: '384',
  };
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function resultText(result: any): string {
  return result.content[0].text as string;
}

interface Running {
  client: Client;
  stop: () => Promise<void>;
}

/** Resolves with the base URL once the HTTP server logs the port it bound. */
function waitForListening(child: ChildProcessWithoutNullStreams): Promise<string> {
  return new Promise((resolve, reject) => {
    let stderr = '';
    const timer = setTimeout(
      () => reject(new Error(`server did not start within 10s; stderr:\n${stderr}`)),
      10_000,
    );
    child.stderr.on('data', (chunk: Buffer) => {
      stderr += chunk.toString();
      const match = /on (http:\/\/127\.0\.0\.1:\d+\/mcp)/.exec(stderr);
      if (match) {
        clearTimeout(timer);
        resolve(match[1]!);
      }
    });
    child.once('exit', (code) => {
      clearTimeout(timer);
      reject(new Error(`server exited with code ${code} before listening; stderr:\n${stderr}`));
    });
  });
}

const transports: Array<{
  name: string;
  start: (vaultArgs: string[], dataDir: string) => Promise<Running>;
}> = [
  {
    name: 'stdio',
    async start(vaultArgs, dataDir) {
      const transport = new StdioClientTransport({
        command: process.execPath,
        args: [cliPath, 'server', ...vaultArgs],
        env: testEnv(dataDir),
        stderr: 'pipe',
      });
      const client = new Client({ name: 'multi-vault-test', version: '0.0.0' });
      await client.connect(transport);
      return { client, stop: () => client.close() };
    },
  },
  {
    name: 'http',
    async start(vaultArgs, dataDir) {
      const child = spawn(
        process.execPath,
        [cliPath, 'server', '--transport', 'http', '--listen', '127.0.0.1:0', ...vaultArgs],
        { env: testEnv(dataDir), stdio: ['pipe', 'pipe', 'pipe'] },
      );
      const url = await waitForListening(child);
      const client = new Client({ name: 'multi-vault-test', version: '0.0.0' });
      await client.connect(new StreamableHTTPClientTransport(new URL(url)));
      return {
        client,
        async stop() {
          await client.close();
          child.kill('SIGTERM');
          const [code] = (await once(child, 'exit')) as [number | null];
          expect(code).toBe(0);
        },
      };
    },
  },
];

describe.each(transports)('obsidian-brain server over $name, two vaults', ({ start }) => {
  let alpha: string;
  let beta: string;
  let dataDir: string;
  let running: Running;

  beforeAll(async () => {
    alpha = mkdtempSync(join(tmpdir(), 'ob-mv-alpha-'));
    beta = mkdtempSync(join(tmpdir(), 'ob-mv-beta-'));
    dataDir = mkdtempSync(join(tmpdir(), 'ob-mv-data-'));
    writeFileSync(join(alpha, 'shared.md'), '# Alpha shared\n');
    writeFileSync(join(beta, 'shared.md'), '# Beta shared\n');
    running = await start(['--vault', `alpha=${alpha}`, '--vault', `beta=${beta}`], dataDir);
  }, 20_000);

  afterAll(async () => {
    await running?.stop();
    for (const dir of [alpha, beta, dataDir]) rmSync(dir, { recursive: true, force: true });
  }, 15_000);

  it('requires a vault argument, limited to the configured names, on every tool but list_vaults', async () => {
    const { tools } = await running.client.listTools();
    expect(tools).toHaveLength(EXPOSED_TOOL_COUNT);
    for (const tool of tools) {
      if (tool.name === 'list_vaults') {
        expect(tool.inputSchema.required ?? []).not.toContain('vault');
        continue;
      }
      expect(tool.inputSchema.required).toContain('vault');
      expect(tool.inputSchema.properties?.vault).toMatchObject({ enum: ['alpha', 'beta'] });
    }
  });

  it('lists both vaults', async () => {
    const result = await running.client.callTool({ name: 'list_vaults', arguments: {} });
    expect(result.isError).toBeFalsy();
    const vaults = JSON.parse(resultText(result)) as Array<{ name: string; path: string }>;
    expect(vaults.map((v) => [v.name, v.path])).toEqual([
      ['alpha', alpha],
      ['beta', beta],
    ]);
  });

  it('keeps each vault index in <DATA_DIR>/<name>', () => {
    expect(existsSync(join(dataDir, 'alpha', 'kg.db'))).toBe(true);
    expect(existsSync(join(dataDir, 'beta', 'kg.db'))).toBe(true);
  });

  it('writes to the named vault only', async () => {
    const result = await running.client.callTool({
      name: 'create_note',
      arguments: { vault: 'alpha', title: 'Only in alpha', content: 'alpha body' },
    });
    expect(result.isError).toBeFalsy();
    expect(readFileSync(join(alpha, 'Only in alpha.md'), 'utf-8')).toContain('alpha body');
    expect(existsSync(join(beta, 'Only in alpha.md'))).toBe(false);
  });

  it('rejects a call without a vault or with an unknown vault', async () => {
    for (const args of [{ title: 'No vault' }, { vault: 'gamma', title: 'Unknown vault' }]) {
      const result = await running.client
        .callTool({ name: 'create_note', arguments: args })
        .catch((err: unknown) => ({ isError: true, content: [{ text: String(err) }] }));
      expect(result.isError).toBe(true);
      expect(resultText(result)).toMatch(/vault/i);
    }
    expect(existsSync(join(alpha, 'No vault.md'))).toBe(false);
    expect(existsSync(join(beta, 'No vault.md'))).toBe(false);
  });

  it('applies an edit preview only in the vault that made it', async () => {
    for (const vault of ['alpha', 'beta']) {
      const created = await running.client.callTool({
        name: 'create_note',
        arguments: { vault, title: 'Preview target', content: 'original' },
      });
      expect(created.isError).toBeFalsy();
    }

    const preview = await running.client.callTool({
      name: 'edit_note',
      arguments: {
        vault: 'alpha',
        name: 'Preview target',
        mode: 'append',
        content: '\nfrom alpha',
        dryRun: true,
      },
    });
    expect(preview.isError).toBeFalsy();
    const { previewId } = JSON.parse(resultText(preview)) as { previewId: string };

    const crossVault = await running.client.callTool({
      name: 'apply_edit_preview',
      arguments: { vault: 'beta', previewId },
    });
    expect(crossVault.isError).toBe(true);
    expect(resultText(crossVault)).toMatch(/not found or expired/);
    expect(readFileSync(join(beta, 'Preview target.md'), 'utf-8')).not.toContain('from alpha');

    const sameVault = await running.client.callTool({
      name: 'apply_edit_preview',
      arguments: { vault: 'alpha', previewId },
    });
    expect(sameVault.isError).toBeFalsy();
    expect(readFileSync(join(alpha, 'Preview target.md'), 'utf-8')).toContain('from alpha');
  });
});

describe('obsidian-brain server over http, routing', () => {
  let vault: string;
  let dataDir: string;
  let child: ChildProcessWithoutNullStreams;
  let url: string;

  beforeAll(async () => {
    vault = mkdtempSync(join(tmpdir(), 'ob-mv-route-'));
    dataDir = mkdtempSync(join(tmpdir(), 'ob-mv-route-data-'));
    child = spawn(
      process.execPath,
      [cliPath, 'server', '--transport', 'http', '--listen', '127.0.0.1:0', '--vault', `notes=${vault}`],
      { env: testEnv(dataDir), stdio: ['pipe', 'pipe', 'pipe'] },
    );
    url = await waitForListening(child);
  }, 20_000);

  afterAll(async () => {
    if (child.exitCode === null) {
      child.kill('SIGKILL');
      await once(child, 'exit');
    }
    for (const dir of [vault, dataDir]) rmSync(dir, { recursive: true, force: true });
  });

  it('answers 404 outside /mcp', async () => {
    const base = url.replace(/\/mcp$/, '');
    expect((await fetch(`${base}/notes/mcp`, { method: 'POST' })).status).toBe(404);
    expect((await fetch(`${base}/`)).status).toBe(404);
  });

  it('answers 405 for GET and DELETE, because the transport is stateless', async () => {
    for (const method of ['GET', 'DELETE']) {
      const res = await fetch(url, { method });
      expect(res.status).toBe(405);
      expect(res.headers.get('allow')).toBe('POST');
    }
  });
});

describe('obsidian-brain server, argument errors', () => {
  let dataDir: string;

  beforeAll(() => {
    dataDir = mkdtempSync(join(tmpdir(), 'ob-mv-args-'));
  });

  afterAll(() => {
    rmSync(dataDir, { recursive: true, force: true });
  });

  async function runToExit(args: string[]): Promise<{ code: number | null; stderr: string }> {
    const child = spawn(process.execPath, [cliPath, 'server', ...args], {
      env: testEnv(dataDir),
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    let stderr = '';
    child.stderr.on('data', (chunk: Buffer) => {
      stderr += chunk.toString();
    });
    const [code] = (await once(child, 'exit')) as [number | null];
    return { code, stderr };
  }

  it('requires at least one --vault', async () => {
    const { code, stderr } = await runToExit([]);
    expect(code).not.toBe(0);
    expect(stderr).toMatch(/at least one vault with --vault/);
  });

  it.each([['no-equals-sign'], ['=/path'], ['bad name=/path'], ['name=']])(
    'rejects the vault spec "%s"',
    async (spec) => {
      const { code, stderr } = await runToExit(['--vault', spec]);
      expect(code).not.toBe(0);
      expect(stderr).toMatch(/<name>=<path>/);
    },
  );

  it('rejects a vault name given twice', async () => {
    const { code, stderr } = await runToExit(['--vault', 'a=/one', '--vault', 'a=/two']);
    expect(code).not.toBe(0);
    expect(stderr).toMatch(/"a" is given more than once/);
  });

  it('rejects an unknown transport', async () => {
    const { code, stderr } = await runToExit(['--vault', 'a=/one', '--transport', 'sse']);
    expect(code).not.toBe(0);
    expect(stderr).toMatch(/stdio, http/);
  });

  it('rejects --listen without --transport http', async () => {
    const { code, stderr } = await runToExit(['--vault', 'a=/one', '--listen', '127.0.0.1:0']);
    expect(code).not.toBe(0);
    expect(stderr).toMatch(/--listen only applies with --transport http/);
  });

  it('rejects a malformed --listen', async () => {
    const { code, stderr } = await runToExit([
      '--vault',
      'a=/one',
      '--transport',
      'http',
      '--listen',
      'nope',
    ]);
    expect(code).not.toBe(0);
    expect(stderr).toMatch(/--listen expects <host>:<port>/);
  });
});
