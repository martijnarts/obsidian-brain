import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';
import { createRequire } from 'node:module';
import { join } from 'node:path';
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StreamableHTTPServerTransport } from '@modelcontextprotocol/sdk/server/streamableHttp.js';
import { createContext, type ServerContext } from './context.js';
import { allNodeIds } from './store/nodes.js';
import { startWatcher, type WatcherHandle } from './pipeline/watcher.js';
import {
  registerTools,
  runStartupIndex,
  closeContext,
  readWatcherOptsFromEnv,
} from './server.js';
import { debugLog } from './util/debug-log.js';
import { logger } from './util/logger.js';

debugLog('module-load: src/http-server.ts');

const pkg = createRequire(import.meta.url)('../package.json') as { version: string };

export interface VaultSpec {
  /** URL segment: the vault is served at `/<name>/mcp`. */
  name: string;
  vaultPath: string;
}

export interface HttpServerOptions {
  host: string;
  port: number;
  vaults: VaultSpec[];
  /** Each vault keeps its index in `<dataDir>/<name>`. */
  dataDir: string;
}

interface ServedVault {
  name: string;
  ctx: ServerContext;
  watcher: WatcherHandle | null;
}

const VAULT_PATH_RE = /^\/([^/]+)\/mcp\/?$/;

/**
 * Serves several vaults from one process over streamable HTTP, each at
 * `/<name>/mcp`. Every vault has its own context: index, graph, watcher and
 * tools. Nothing crosses between vaults.
 *
 * The transport runs stateless. Each POST gets a fresh McpServer bound to
 * the vault's context, because no tool sends server-initiated messages.
 */
export interface HttpServerHandle {
  /** The bound port; differs from the requested one when that was 0. */
  port: number;
  /** Stops accepting requests, then closes every vault's watcher, embedder and DB. */
  close: () => Promise<void>;
}

export async function startHttpServer(opts: HttpServerOptions): Promise<HttpServerHandle> {
  const served: ServedVault[] = [];
  let httpServer: Server | null = null;
  let closing: Promise<void> | null = null;
  const close = (): Promise<void> => {
    closing ??= (async () => {
      if (httpServer?.listening) {
        await new Promise<void>((resolve) => {
          httpServer!.close(() => resolve());
          httpServer!.closeAllConnections();
        });
      }
      for (const vault of served) {
        try {
          await vault.watcher?.close();
          await closeContext(vault.ctx);
        } catch (err) {
          logger.warn(`teardown error for vault "${vault.name}" (ignored): ${err}`, {
            vault: vault.name,
            error: String(err),
          });
        }
      }
    })();
    return closing;
  };

  try {
    for (const spec of opts.vaults) {
      const ctx = await createContext({
        vaultPath: spec.vaultPath,
        dataDir: join(opts.dataDir, spec.name),
      });
      served.push({ name: spec.name, ctx, watcher: null });
    }
    const byName = new Map(served.map((v) => [v.name, v]));

    httpServer = createServer((req, res) => {
      void handleRequest(byName, req, res).catch((err: unknown) => {
        logger.error(`request failed: ${String(err)}`, { error: String(err) });
        if (!res.headersSent) {
          sendJsonRpcError(res, 500, -32603, 'Internal server error');
        }
      });
    });
    await new Promise<void>((resolve, reject) => {
      httpServer!.once('error', reject);
      httpServer!.listen(opts.port, opts.host, () => resolve());
    });
  } catch (err) {
    await close();
    throw err;
  }
  // Port 0 picks a free port; report the one actually bound.
  const port = (httpServer.address() as AddressInfo).port;
  logger.info(
    `serving ${served.map((v) => `/${v.name}/mcp`).join(', ')} on http://${opts.host}:${port}`,
    { vaults: served.map((v) => v.name) },
  );

  if (process.env.OBSIDIAN_BRAIN_NO_WATCH !== '1') {
    for (const vault of served) {
      vault.watcher = startWatcher(vault.ctx, readWatcherOptsFromEnv());
    }
  }

  // Index one vault at a time, so the memory peaks of two full indexes never
  // add up. A tool call on a vault that waits its turn still works: it loads
  // the embedder on demand.
  void (async () => {
    for (const vault of served) {
      if (closing) return;
      await runStartupIndex(vault.ctx, allNodeIds(vault.ctx.db).length === 0);
      await vault.ctx.pendingReindex;
    }
  })();

  return { port, close };
}

/**
 * Runs `startHttpServer` until SIGINT or SIGTERM. The handlers are armed
 * before any await, for the same reason as in `startServer`.
 */
export async function runHttpServer(opts: HttpServerOptions): Promise<void> {
  let handle: HttpServerHandle | null = null;
  let stopping = false;
  const shutdown = async (reason: string): Promise<void> => {
    if (stopping) return;
    stopping = true;
    logger.info(`shutting down (${reason}).`, { reason });
    await handle?.close();
    process.exitCode = 0;
    setTimeout(() => process.exit(0), 4_000).unref();
  };
  process.on('SIGINT', () => void shutdown('SIGINT'));
  process.on('SIGTERM', () => void shutdown('SIGTERM'));
  handle = await startHttpServer(opts);
  if (stopping) await handle.close();
}

async function handleRequest(
  vaults: Map<string, ServedVault>,
  req: IncomingMessage,
  res: ServerResponse,
): Promise<void> {
  const path = new URL(req.url ?? '/', 'http://localhost').pathname;
  const match = VAULT_PATH_RE.exec(path);
  const vault = match ? vaults.get(decodeURIComponent(match[1]!)) : undefined;
  if (!vault) {
    res.writeHead(404, { 'Content-Type': 'text/plain' }).end('Not found\n');
    return;
  }
  // Stateless: there is no session to stream to or to delete.
  if (req.method !== 'POST') {
    res.setHeader('Allow', 'POST');
    sendJsonRpcError(res, 405, -32000, 'Method not allowed.');
    return;
  }

  const server = new McpServer({ name: 'obsidian-brain', version: pkg.version });
  registerTools(server, vault.ctx);
  const transport = new StreamableHTTPServerTransport({ sessionIdGenerator: undefined });
  res.on('close', () => {
    void transport.close();
    void server.close();
  });
  await server.connect(transport);
  await transport.handleRequest(req, res);
}

function sendJsonRpcError(res: ServerResponse, status: number, code: number, message: string): void {
  res
    .writeHead(status, { 'Content-Type': 'application/json' })
    .end(JSON.stringify({ jsonrpc: '2.0', error: { code, message }, id: null }));
}
