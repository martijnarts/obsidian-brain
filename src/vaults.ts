import { join } from 'node:path';
import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { createContext, type ServerContext } from './context.js';
import { allNodeIds } from './store/nodes.js';
import { startWatcher, type WatcherHandle, type WatcherOptions } from './pipeline/watcher.js';
import { registerTool } from './tools/register.js';
import { debugLog } from './util/debug-log.js';
import { logger } from './util/logger.js';

debugLog('module-load: src/vaults.ts');

export interface VaultSpec {
  /** The value of the `vault` argument that selects this vault. */
  name: string;
  vaultPath: string;
}

export interface Vault {
  name: string;
  ctx: ServerContext;
  watcher: WatcherHandle | null;
}

/** Registers one vault's tools on `server`. `registerTools` in server.ts. */
export type ToolRegistrar = (server: McpServer, ctx: ServerContext) => void;

/**
 * Opens a context per vault. Each vault keeps its index in
 * `<dataDir>/<name>`. If one vault fails to open, the ones already open are
 * closed again before the error propagates.
 */
export async function openVaults(
  specs: VaultSpec[],
  dataDir: string,
  close: (ctx: ServerContext) => Promise<void>,
): Promise<Vault[]> {
  const vaults: Vault[] = [];
  try {
    for (const spec of specs) {
      const ctx = await createContext({
        vaultPath: spec.vaultPath,
        dataDir: join(dataDir, spec.name),
      });
      vaults.push({ name: spec.name, ctx, watcher: null });
    }
  } catch (err) {
    await closeVaults(vaults, close);
    throw err;
  }
  return vaults;
}

/** Closes every vault's watcher and context. One failure does not stop the rest. */
export async function closeVaults(
  vaults: Vault[],
  close: (ctx: ServerContext) => Promise<void>,
): Promise<void> {
  for (const vault of vaults) {
    try {
      await vault.watcher?.close();
      await close(vault.ctx);
    } catch (err) {
      logger.warn(`teardown error for vault "${vault.name}" (ignored): ${err}`, {
        vault: vault.name,
        error: String(err),
      });
    }
  }
}

/** Starts a watcher per vault unless OBSIDIAN_BRAIN_NO_WATCH=1. */
export function startVaultWatchers(vaults: Vault[], opts: WatcherOptions): void {
  if (process.env.OBSIDIAN_BRAIN_NO_WATCH === '1') return;
  for (const vault of vaults) {
    vault.watcher = startWatcher(vault.ctx, opts);
  }
}

/**
 * Runs the startup index of each vault in turn, so the memory peaks of two
 * full indexes never add up. A tool call on a vault that waits its turn
 * still works: it loads the embedder on demand. Stops early once
 * `stopped()` returns true.
 */
export async function indexVaultsInTurn(
  vaults: Vault[],
  runStartupIndex: (ctx: ServerContext, dbIsEmpty: boolean) => Promise<void>,
  stopped: () => boolean,
): Promise<void> {
  for (const vault of vaults) {
    if (stopped()) return;
    await runStartupIndex(vault.ctx, allNodeIds(vault.ctx.db).length === 0);
    await vault.ctx.pendingReindex;
  }
}

type ToolCallback = (args: Record<string, unknown>) => Promise<unknown>;

interface VaultTool {
  description: string;
  shape: Record<string, z.ZodType>;
  handlers: Map<string, ToolCallback>;
}

/**
 * The tools of every vault, merged into one set. Each tool takes a required
 * `vault` argument and forwards the call to that vault's handler. Build it
 * once; `register` is cheap enough to run for every HTTP request.
 */
export class VaultTools {
  private readonly tools = new Map<string, VaultTool>();
  private readonly vaultArg: z.ZodType;

  constructor(
    private readonly vaults: Vault[],
    registerVaultTools: ToolRegistrar,
  ) {
    const names = vaults.map((v) => v.name);
    if (names.length === 0) throw new Error('VaultTools needs at least one vault.');
    this.vaultArg = z
      .enum(names as [string, ...string[]])
      .describe('The vault to work in. `list_vaults` describes each vault.');

    for (const vault of vaults) {
      const capture = {
        tool: (name: string, description: string, shape: Record<string, z.ZodType>, cb: ToolCallback) => {
          let tool = this.tools.get(name);
          if (!tool) {
            if ('vault' in shape) {
              throw new Error(`Tool "${name}" already has a "vault" argument.`);
            }
            tool = { description, shape, handlers: new Map() };
            this.tools.set(name, tool);
          }
          tool.handlers.set(vault.name, cb);
        },
      };
      registerVaultTools(capture as unknown as McpServer, vault.ctx);
    }
  }

  /** The tool names, `list_vaults` included. */
  names(): string[] {
    return ['list_vaults', ...this.tools.keys()];
  }

  register(server: McpServer): void {
    registerTool(
      server,
      'list_vaults',
      'List the vaults this server serves. Every other tool takes one of these names as its required `vault` argument.',
      {},
      async () => this.describeVaults(),
    );
    for (const [name, tool] of this.tools) {
      const cb = async (args: Record<string, unknown>) => {
        const { vault, ...rest } = args;
        // The enum schema has already rejected unknown names.
        return tool.handlers.get(vault as string)!(rest);
      };
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (server as any).tool(name, tool.description, { vault: this.vaultArg, ...tool.shape }, cb);
    }
  }

  private describeVaults() {
    return this.vaults.map(({ name, ctx }) => ({
      name,
      path: ctx.config.vaultPath,
      notes: allNodeIds(ctx.db).length,
      embedderReady: ctx.embedderReady(),
      reindexInProgress: ctx.reindexInProgress,
      ...(ctx.initError !== undefined ? { initError: String(ctx.initError) } : {}),
    }));
  }
}
