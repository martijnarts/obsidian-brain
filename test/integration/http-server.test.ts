/**
 * Integration tests for `obsidian-brain http`: one process serves several
 * vaults over streamable HTTP, each at `/<name>/mcp`.
 *
 * The tests spawn the built CLI with two temp vaults and talk to it with the
 * SDK's HTTP client. The embedder points at an unreachable Ollama, so no
 * model loads; the tests cover routing, vault isolation and shutdown.
 */

import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { once } from 'node:events';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';

const cliPath = join(process.cwd(), 'dist', 'cli', 'index.js');

const testEnv = {
  ...process.env,
  OBSIDIAN_BRAIN_NO_WATCH: '1',
  OBSIDIAN_BRAIN_NO_CATCHUP: '1',
  EMBEDDING_PROVIDER: 'ollama',
  OLLAMA_BASE_URL: 'http://127.0.0.1:1', // unreachable; background init fails fast
  OLLAMA_EMBEDDING_DIM: '384',
};

function spawnCli(args: string[], dataDir: string): ChildProcessWithoutNullStreams {
  return spawn(process.execPath, [cliPath, ...args], {
    env: { ...testEnv, DATA_DIR: dataDir },
    stdio: ['pipe', 'pipe', 'pipe'],
  });
}

/** Resolves with the base URL once the server logs the port it bound. */
function waitForListening(child: ChildProcessWithoutNullStreams): Promise<string> {
  return new Promise((resolve, reject) => {
    let stderr = '';
    const timer = setTimeout(
      () => reject(new Error(`server did not start within 10s; stderr:\n${stderr}`)),
      10_000,
    );
    child.stderr.on('data', (chunk: Buffer) => {
      stderr += chunk.toString();
      const match = /on (http:\/\/127\.0\.0\.1:\d+)/.exec(stderr);
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

async function connect(url: string): Promise<Client> {
  const client = new Client({ name: 'http-server-test', version: '0.0.0' });
  await client.connect(new StreamableHTTPClientTransport(new URL(url)));
  return client;
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function resultText(result: any): string {
  return result.content[0].text as string;
}

describe.sequential('obsidian-brain http — two vaults', () => {
  let alpha: string;
  let beta: string;
  let dataDir: string;
  let child: ChildProcessWithoutNullStreams;
  let base: string;
  let stderr = '';

  beforeAll(async () => {
    alpha = mkdtempSync(join(tmpdir(), 'ob-http-alpha-'));
    beta = mkdtempSync(join(tmpdir(), 'ob-http-beta-'));
    dataDir = mkdtempSync(join(tmpdir(), 'ob-http-data-'));
    writeFileSync(join(alpha, 'shared.md'), '# Alpha shared\n');
    writeFileSync(join(beta, 'shared.md'), '# Beta shared\n');

    child = spawnCli(
      ['http', '--listen', '127.0.0.1:0', '--vault', `alpha=${alpha}`, '--vault', `beta=${beta}`],
      dataDir,
    );
    child.stderr.on('data', (chunk: Buffer) => {
      stderr += chunk.toString();
    });
    base = await waitForListening(child);
  }, 20_000);

  afterAll(async () => {
    if (child.exitCode === null) {
      child.kill('SIGKILL');
      await once(child, 'exit');
    }
    for (const dir of [alpha, beta, dataDir]) rmSync(dir, { recursive: true, force: true });
  });

  it('serves the full tool list at each vault path', async () => {
    for (const name of ['alpha', 'beta']) {
      const client = await connect(`${base}/${name}/mcp`);
      const { tools } = await client.listTools();
      expect(tools.map((t) => t.name)).toEqual(
        expect.arrayContaining(['search', 'read_note', 'create_note', 'edit_note', 'index_status']),
      );
      expect(tools).toHaveLength(18);
      await client.close();
    }
  });

  it('keeps each vault index in <DATA_DIR>/<name>', () => {
    expect(existsSync(join(dataDir, 'alpha', 'kg.db'))).toBe(true);
    expect(existsSync(join(dataDir, 'beta', 'kg.db'))).toBe(true);
  });

  it('writes through a vault path land only in that vault', async () => {
    const client = await connect(`${base}/alpha/mcp`);
    const result = await client.callTool({
      name: 'create_note',
      arguments: { title: 'Only in alpha', content: 'alpha body' },
    });
    expect(result.isError).toBeFalsy();
    await client.close();

    expect(readFileSync(join(alpha, 'Only in alpha.md'), 'utf-8')).toContain('alpha body');
    expect(existsSync(join(beta, 'Only in alpha.md'))).toBe(false);
  });

  it('applies an edit preview only through the vault that made it', async () => {
    const alphaClient = await connect(`${base}/alpha/mcp`);
    const betaClient = await connect(`${base}/beta/mcp`);

    // create_note registers the note in the index, so edit_note can find it.
    for (const client of [alphaClient, betaClient]) {
      const created = await client.callTool({
        name: 'create_note',
        arguments: { title: 'Preview target', content: 'original' },
      });
      expect(created.isError).toBeFalsy();
    }

    const preview = await alphaClient.callTool({
      name: 'edit_note',
      arguments: { name: 'Preview target', mode: 'append', content: '\nfrom alpha', dryRun: true },
    });
    expect(preview.isError).toBeFalsy();
    const { previewId } = JSON.parse(resultText(preview)) as { previewId: string };

    const crossVault = await betaClient.callTool({
      name: 'apply_edit_preview',
      arguments: { previewId },
    });
    expect(crossVault.isError).toBe(true);
    expect(resultText(crossVault)).toMatch(/not found or expired/);
    expect(readFileSync(join(beta, 'Preview target.md'), 'utf-8')).not.toContain('from alpha');

    const sameVault = await alphaClient.callTool({
      name: 'apply_edit_preview',
      arguments: { previewId },
    });
    expect(sameVault.isError).toBeFalsy();
    expect(readFileSync(join(alpha, 'Preview target.md'), 'utf-8')).toContain('from alpha');

    await alphaClient.close();
    await betaClient.close();
  });

  it('answers 404 for an unknown vault and for other paths', async () => {
    expect((await fetch(`${base}/gamma/mcp`, { method: 'POST' })).status).toBe(404);
    expect((await fetch(`${base}/alpha`, { method: 'POST' })).status).toBe(404);
    expect((await fetch(`${base}/`)).status).toBe(404);
  });

  it('answers 405 for GET and DELETE, because the transport is stateless', async () => {
    for (const method of ['GET', 'DELETE']) {
      const res = await fetch(`${base}/alpha/mcp`, { method });
      expect(res.status).toBe(405);
      expect(res.headers.get('allow')).toBe('POST');
    }
  });

  it('shuts down cleanly on SIGTERM', async () => {
    child.kill('SIGTERM');
    const [code] = (await Promise.race([
      once(child, 'exit'),
      new Promise((_, reject) =>
        setTimeout(() => reject(new Error('server did not exit within 6s of SIGTERM')), 6_000),
      ),
    ])) as [number | null];
    expect(code).toBe(0);
    expect(stderr).toMatch(/shutting down \(SIGTERM\)/);
  }, 15_000);
});

describe('obsidian-brain http — argument errors', () => {
  let dataDir: string;

  beforeAll(() => {
    dataDir = mkdtempSync(join(tmpdir(), 'ob-http-args-'));
  });

  afterAll(() => {
    rmSync(dataDir, { recursive: true, force: true });
  });

  async function runToExit(args: string[]): Promise<{ code: number | null; stderr: string }> {
    const child = spawnCli(['http', ...args], dataDir);
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
    expect(stderr).toMatch(/--vault/);
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

  it('rejects a malformed --listen', async () => {
    const { code, stderr } = await runToExit(['--vault', 'a=/one', '--listen', 'nope']);
    expect(code).not.toBe(0);
    expect(stderr).toMatch(/--listen expects <host>:<port>/);
  });
});
