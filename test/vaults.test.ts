/**
 * Unit tests for src/vaults.ts: the tool router that adds a required
 * `vault` argument to every tool, `list_vaults`, and the helpers that open,
 * index and close a set of vaults.
 */

import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import type { ServerContext } from '../src/context.js';
import { registerTool } from '../src/tools/register.js';
import {
  closeVaults,
  indexVaultsInTurn,
  openVaults,
  startVaultWatchers,
  VaultTools,
  type Vault,
} from '../src/vaults.js';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Cb = (args: any) => Promise<any>;

interface Registered {
  description: string;
  shape: Record<string, z.ZodType>;
  cb: Cb;
}

function recordingServer(): { server: McpServer; tools: Map<string, Registered> } {
  const tools = new Map<string, Registered>();
  const server = {
    tool(name: string, description: string, shape: Record<string, z.ZodType>, cb: Cb) {
      tools.set(name, { description, shape, cb });
    },
  } as unknown as McpServer;
  return { server, tools };
}

function fakeVault(name: string, notes: string[] = []): Vault {
  const ctx = {
    config: { vaultPath: `/vaults/${name}` },
    db: {
      prepare: () => ({ all: () => notes.map((id) => ({ id })), pluck: () => ({ all: () => notes }) }),
    },
    embedderReady: () => false,
    reindexInProgress: false,
    initError: undefined,
    pendingReindex: Promise.resolve(),
  } as unknown as ServerContext;
  return { name, ctx, watcher: null };
}

/** A registrar with one tool that reports which vault served it. */
function echoRegistrar(server: McpServer, ctx: ServerContext): void {
  registerTool(server, 'echo', 'Echo the arguments.', { text: z.string() }, async (args) => ({
    vaultPath: ctx.config.vaultPath,
    args,
  }));
}

function parse(result: { content: Array<{ text: string }> }): unknown {
  return JSON.parse(result.content[0]!.text);
}

describe('VaultTools', () => {
  it('adds a required vault enum to every tool and keeps the original arguments', () => {
    const { server, tools } = recordingServer();
    new VaultTools([fakeVault('alpha'), fakeVault('beta')], echoRegistrar).register(server);

    const echo = tools.get('echo')!;
    expect(echo.description).toBe('Echo the arguments.');
    const schema = z.object(echo.shape);
    expect(schema.safeParse({ vault: 'alpha', text: 'hi' }).success).toBe(true);
    expect(schema.safeParse({ text: 'hi' }).success).toBe(false);
    expect(schema.safeParse({ vault: 'gamma', text: 'hi' }).success).toBe(false);
    expect(schema.safeParse({ vault: 'alpha' }).success).toBe(false);
  });

  it('forwards each call to the named vault, without the vault argument', async () => {
    const { server, tools } = recordingServer();
    new VaultTools([fakeVault('alpha'), fakeVault('beta')], echoRegistrar).register(server);

    for (const name of ['alpha', 'beta']) {
      const result = await tools.get('echo')!.cb({ vault: name, text: 'hi' });
      expect(parse(result)).toEqual({ vaultPath: `/vaults/${name}`, args: { text: 'hi' } });
    }
  });

  it('registers list_vaults, which takes no arguments', async () => {
    const { server, tools } = recordingServer();
    new VaultTools([fakeVault('alpha', ['a.md', 'b.md']), fakeVault('beta')], echoRegistrar).register(
      server,
    );

    const listVaults = tools.get('list_vaults')!;
    expect(listVaults.shape).toEqual({});
    expect(parse(await listVaults.cb({}))).toEqual([
      { name: 'alpha', path: '/vaults/alpha', notes: 2, embedderReady: false, reindexInProgress: false },
      { name: 'beta', path: '/vaults/beta', notes: 0, embedderReady: false, reindexInProgress: false },
    ]);
  });

  it('reports a vault whose startup failed', async () => {
    const vault = fakeVault('alpha');
    vault.ctx.initError = new Error('embedder unavailable');
    const { server, tools } = recordingServer();
    new VaultTools([vault], echoRegistrar).register(server);

    expect(parse(await tools.get('list_vaults')!.cb({}))).toEqual([
      expect.objectContaining({ name: 'alpha', initError: 'Error: embedder unavailable' }),
    ]);
  });

  it('names list_vaults first, then the routed tools', () => {
    const tools = new VaultTools([fakeVault('alpha')], echoRegistrar);
    expect(tools.names()).toEqual(['list_vaults', 'echo']);
  });

  it('can register on a fresh server for every HTTP request', () => {
    const tools = new VaultTools([fakeVault('alpha')], echoRegistrar);
    for (let i = 0; i < 3; i++) {
      const { server, tools: registered } = recordingServer();
      tools.register(server);
      expect([...registered.keys()]).toEqual(['list_vaults', 'echo']);
    }
  });

  it('refuses no vaults at all', () => {
    expect(() => new VaultTools([], echoRegistrar)).toThrow(/at least one vault/);
  });

  it('refuses a tool that already has a vault argument', () => {
    const clash = (server: McpServer) =>
      registerTool(server, 'clash', 'Clash.', { vault: z.string() }, async () => null);
    expect(() => new VaultTools([fakeVault('alpha')], clash)).toThrow(
      /"clash" already has a "vault" argument/,
    );
  });
});

describe('vault lifecycle helpers', () => {
  let dataDir: string;
  let vaultDir: string;

  beforeEach(() => {
    vi.stubEnv('EMBEDDING_PROVIDER', 'ollama');
    vi.stubEnv('OLLAMA_BASE_URL', 'http://127.0.0.1:1');
    vi.stubEnv('OLLAMA_EMBEDDING_DIM', '384');
    dataDir = mkdtempSync(join(tmpdir(), 'ob-vaults-data-'));
    vaultDir = mkdtempSync(join(tmpdir(), 'ob-vaults-vault-'));
  });

  afterEach(() => {
    vi.unstubAllEnvs();
    for (const dir of [dataDir, vaultDir]) rmSync(dir, { recursive: true, force: true });
  });

  it('opens a context per vault under <dataDir>/<name>', async () => {
    const close = vi.fn(async (ctx: ServerContext) => ctx.db.close());
    const vaults = await openVaults(
      [
        { name: 'one', vaultPath: vaultDir },
        { name: 'two', vaultPath: vaultDir },
      ],
      dataDir,
      close,
    );
    expect(vaults.map((v) => [v.name, v.ctx.config.dataDir])).toEqual([
      ['one', join(dataDir, 'one')],
      ['two', join(dataDir, 'two')],
    ]);
    await closeVaults(vaults, close);
    expect(close).toHaveBeenCalledTimes(2);
  });

  it('closes the vaults it opened when a later one fails to open', async () => {
    const close = vi.fn(async (ctx: ServerContext) => ctx.db.close());
    // A regular file where the second vault's data dir should be.
    writeFileSync(join(dataDir, 'two'), '');
    await expect(
      openVaults(
        [
          { name: 'one', vaultPath: vaultDir },
          { name: 'two', vaultPath: vaultDir },
        ],
        dataDir,
        close,
      ),
    ).rejects.toThrow();
    expect(close).toHaveBeenCalledTimes(1);
    expect(close.mock.calls[0]![0].config.dataDir).toBe(join(dataDir, 'one'));
  });

  it('keeps closing the other vaults when one fails to close', async () => {
    const failing = fakeVault('alpha');
    const healthy = fakeVault('beta');
    const close = vi.fn(async (ctx: ServerContext) => {
      if (ctx === failing.ctx) throw new Error('close failed');
    });
    await closeVaults([failing, healthy], close);
    expect(close).toHaveBeenCalledWith(healthy.ctx);
  });

  it('closes each watcher before its context', async () => {
    const order: string[] = [];
    const vault = fakeVault('alpha');
    vault.watcher = { close: async () => void order.push('watcher') } as Vault['watcher'];
    await closeVaults([vault], async () => void order.push('context'));
    expect(order).toEqual(['watcher', 'context']);
  });

  it('starts no watchers when OBSIDIAN_BRAIN_NO_WATCH=1', () => {
    vi.stubEnv('OBSIDIAN_BRAIN_NO_WATCH', '1');
    const vaults = [fakeVault('alpha')];
    startVaultWatchers(vaults, {});
    expect(vaults[0]!.watcher).toBeNull();
  });

  it('indexes vaults one at a time, each after the previous one finished', async () => {
    const order: string[] = [];
    const vaults = [fakeVault('alpha'), fakeVault('beta')];
    const run = vi.fn(async (ctx: ServerContext, dbIsEmpty: boolean) => {
      const name = ctx.config.vaultPath;
      order.push(`start ${name} empty=${dbIsEmpty}`);
      ctx.pendingReindex = new Promise((resolve) =>
        setTimeout(() => {
          order.push(`done ${name}`);
          resolve();
        }, 10),
      );
    });
    await indexVaultsInTurn(vaults, run, () => false);
    expect(order).toEqual([
      'start /vaults/alpha empty=true',
      'done /vaults/alpha',
      'start /vaults/beta empty=true',
      'done /vaults/beta',
    ]);
  });

  it('stops indexing once stopped', async () => {
    let stopped = false;
    const run = vi.fn(async () => {
      stopped = true;
    });
    await indexVaultsInTurn([fakeVault('alpha'), fakeVault('beta')], run, () => stopped);
    expect(run).toHaveBeenCalledTimes(1);
  });
});
