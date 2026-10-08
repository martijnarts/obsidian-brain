/**
 * Integration tests for the `index`, `watch` and `search` subcommands with
 * --vault. They use the same per-vault indexes as `server`, at
 * `<DATA_DIR>/<name>/kg.db`.
 *
 * The tests spawn the built CLI. The embedder points at an unreachable
 * Ollama, so no model loads; notes still get indexed for full-text search.
 */

import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

const cliPath = join(process.cwd(), 'dist', 'cli', 'index.js');

interface Run {
  code: number | null;
  stdout: string;
  stderr: string;
}

describe('index, watch and search with --vault', () => {
  let alpha: string;
  let beta: string;
  let dataDir: string;

  function env(extra: Record<string, string> = {}): NodeJS.ProcessEnv {
    return {
      ...process.env,
      DATA_DIR: dataDir,
      EMBEDDING_PROVIDER: 'ollama',
      OLLAMA_BASE_URL: 'http://127.0.0.1:1', // unreachable; no model loads
      OLLAMA_EMBEDDING_DIM: '384',
      ...extra,
    };
  }

  async function run(args: string[], extraEnv: Record<string, string> = {}): Promise<Run> {
    const child = spawn(process.execPath, [cliPath, ...args], {
      env: env(extraEnv),
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (chunk: Buffer) => (stdout += chunk.toString()));
    child.stderr.on('data', (chunk: Buffer) => (stderr += chunk.toString()));
    const [code] = (await once(child, 'exit')) as [number | null];
    return { code, stdout, stderr };
  }

  beforeAll(() => {
    alpha = mkdtempSync(join(tmpdir(), 'ob-cli-alpha-'));
    beta = mkdtempSync(join(tmpdir(), 'ob-cli-beta-'));
    dataDir = mkdtempSync(join(tmpdir(), 'ob-cli-data-'));
    writeFileSync(join(alpha, 'Lighthouse.md'), '# Lighthouse\n\nThe lighthouse keeper rows to the alpha island every morning.\n');
    writeFileSync(join(beta, 'Harbour.md'), '# Harbour\n\nThe harbour master counts the beta boats every evening.\n');
  });

  afterAll(() => {
    for (const dir of [alpha, beta, dataDir]) rmSync(dir, { recursive: true, force: true });
  });

  it('index builds one index per vault, in <DATA_DIR>/<name>', async () => {
    const result = await run(['index', '--vault', `alpha=${alpha}`, '--vault', `beta=${beta}`]);
    expect(result.code, result.stderr).toBe(0);
    const stats = JSON.parse(result.stdout) as Record<string, { nodesIndexed: number }>;
    expect(Object.keys(stats)).toEqual(['alpha', 'beta']);
    expect(stats.alpha!.nodesIndexed).toBe(1);
    expect(stats.beta!.nodesIndexed).toBe(1);
    expect(existsSync(join(dataDir, 'alpha', 'kg.db'))).toBe(true);
    expect(existsSync(join(dataDir, 'beta', 'kg.db'))).toBe(true);
    expect(existsSync(join(dataDir, 'kg.db'))).toBe(false);
  });

  it('search reads the index of the vault it names, and only that one', async () => {
    const hit = await run(['search', '--vault', `alpha=${alpha}`, '--mode', 'fulltext', 'lighthouse']);
    expect(hit.code, hit.stderr).toBe(0);
    expect(hit.stdout).toContain('Lighthouse.md');

    const miss = await run(['search', '--vault', `beta=${beta}`, '--mode', 'fulltext', 'lighthouse']);
    expect(miss.code, miss.stderr).toBe(0);
    expect(JSON.parse(miss.stdout)).toEqual([]);
  });

  it('search takes exactly one vault', async () => {
    const result = await run([
      'search', '--vault', `alpha=${alpha}`, '--vault', `beta=${beta}`, 'lighthouse',
    ]);
    expect(result.code).not.toBe(0);
    expect(result.stderr).toMatch(/search takes exactly one --vault/);
  });

  it('search rejects an unknown mode', async () => {
    const result = await run(['search', '--vault', `alpha=${alpha}`, '--mode', 'fuzzy', 'lighthouse']);
    expect(result.code).not.toBe(0);
    expect(result.stderr).toMatch(/Unknown --mode 'fuzzy'/);
  });

  it.each([['index'], ['watch'], ['search', 'lighthouse']])(
    '%s requires --vault and ignores VAULT_PATH',
    async (...args) => {
      const result = await run(args, { VAULT_PATH: alpha });
      expect(result.code).not.toBe(0);
      expect(result.stderr).toMatch(/at least one vault with --vault/);
    },
  );

  it('watch watches every vault and shuts down cleanly on SIGTERM', async () => {
    const child = spawn(
      process.execPath,
      [cliPath, 'watch', '--vault', `alpha=${alpha}`, '--vault', `beta=${beta}`],
      { env: env(), stdio: ['pipe', 'pipe', 'pipe'] },
    );
    let stderr = '';
    child.stderr.on('data', (chunk: Buffer) => (stderr += chunk.toString()));
    await new Promise((resolve) => setTimeout(resolve, 1_500));
    child.kill('SIGTERM');
    const [code] = (await once(child, 'exit')) as [number | null];
    expect(code, stderr).toBe(0);
    expect(stderr).toMatch(/shutting down \(SIGTERM\)/);
  }, 15_000);

  it('models refresh-cache clears the per-vault indexes that index built', async () => {
    const result = await run(['models', 'refresh-cache']);
    expect(result.code, result.stderr).toBe(0);
    const parsed = JSON.parse(result.stdout) as { vaults: Array<{ dbPath: string }> };
    expect(parsed.vaults.map((v) => v.dbPath)).toEqual([
      join(dataDir, 'alpha', 'kg.db'),
      join(dataDir, 'beta', 'kg.db'),
    ]);
  });
});
