import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';
import { createRequire } from 'node:module';
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StreamableHTTPServerTransport } from '@modelcontextprotocol/sdk/server/streamableHttp.js';
import {
  registerTools,
  runStartupIndex,
  closeContext,
  readWatcherOptsFromEnv,
  type ServerOptions,
} from './server.js';
import {
  closeVaults,
  indexVaultsInTurn,
  openVaults,
  startVaultWatchers,
  VaultTools,
  type Vault,
} from './vaults.js';
import { debugLog } from './util/debug-log.js';
import { logger } from './util/logger.js';

debugLog('module-load: src/http-server.ts');

const pkg = createRequire(import.meta.url)('../package.json') as { version: string };

export interface HttpServerOptions extends ServerOptions {
  host: string;
  port: number;
}

export interface HttpServerHandle {
  /** The bound port; differs from the requested one when that was 0. */
  port: number;
  /** Stops accepting requests, then closes every vault's watcher, embedder and DB. */
  close: () => Promise<void>;
}

const MCP_PATH_RE = /^\/mcp\/?$/;

/**
 * Serves every vault over streamable HTTP at `/mcp`. Each tool takes a
 * required `vault` argument naming one of `opts.vaults`.
 *
 * The transport runs stateless. Each POST gets a fresh McpServer with the
 * shared tool set, because no tool sends server-initiated messages.
 */
export async function startHttpServer(opts: HttpServerOptions): Promise<HttpServerHandle> {
  let vaults: Vault[] = [];
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
      await closeVaults(vaults, closeContext);
    })();
    return closing;
  };

  try {
    vaults = await openVaults(opts.vaults, opts.dataDir, closeContext);
    const tools = new VaultTools(vaults, registerTools);
    httpServer = createServer((req, res) => {
      void handleRequest(tools, req, res).catch((err: unknown) => {
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
    `serving vaults ${vaults.map((v) => v.name).join(', ')} on http://${opts.host}:${port}/mcp`,
    { vaults: vaults.map((v) => v.name) },
  );

  startVaultWatchers(vaults, readWatcherOptsFromEnv());
  void indexVaultsInTurn(vaults, runStartupIndex, () => closing !== null);

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
  tools: VaultTools,
  req: IncomingMessage,
  res: ServerResponse,
): Promise<void> {
  const path = new URL(req.url ?? '/', 'http://localhost').pathname;
  if (!MCP_PATH_RE.test(path)) {
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
  tools.register(server);
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
