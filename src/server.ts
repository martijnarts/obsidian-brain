import { createRequire } from 'module';
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import type { ServerContext } from './context.js';
import {
  closeVaults,
  indexVaultsInTurn,
  openVaults,
  startVaultWatchers,
  VaultTools,
  type Vault,
  type VaultSpec,
} from './vaults.js';
import { debugLog } from './util/debug-log.js';
import { logger } from './util/logger.js';

debugLog('module-load: src/server.ts');

const pkg = createRequire(import.meta.url)('../package.json') as { version: string };
import { allNodeIds } from './store/nodes.js';


import { registerSearchTool } from './tools/search.js';
import { registerReadNoteTool } from './tools/read-note.js';
import { registerListNotesTool } from './tools/list-notes.js';
import { registerFindNotesByNameTool } from './tools/find-notes-by-name.js';
import { registerGrepVaultTool } from './tools/grep-vault.js';
import { registerQueryNotesTool } from './tools/query-notes.js';
import { registerFindConnectionsTool } from './tools/find-connections.js';
import { registerFindPathBetweenTool } from './tools/find-path-between.js';
import { registerDetectThemesTool } from './tools/detect-themes.js';
import { registerRankNotesTool } from './tools/rank-notes.js';
import { registerCreateNoteTool } from './tools/create-note.js';
import { registerCreateNoteFromTemplateTool } from './tools/create-note-from-template.js';
import { registerEditNoteTool } from './tools/edit-note.js';
import { registerApplyEditPreviewTool } from './tools/apply-edit-preview.js';
import { registerLinkNotesTool } from './tools/link-notes.js';
import { registerMoveNoteTool } from './tools/move-note.js';
import { registerDeleteNoteTool } from './tools/delete-note.js';
import { registerReadCanvasTool } from './tools/read-canvas.js';
import { registerEditCanvasTool } from './tools/edit-canvas.js';
import { registerReindexTool } from './tools/reindex.js';
import { registerActiveNoteTool } from './tools/active-note.js';
import { registerDataviewQueryTool } from './tools/dataview-query.js';
import { registerBaseQueryTool } from './tools/base-query.js';
import { registerIndexStatusTool } from './tools/index-status.js';
import { registerVaultOverviewTool } from './tools/vault-overview.js';
import { registerListTagsTool } from './tools/list-tags.js';
import { registerListBookmarksTool } from './tools/list-bookmarks.js';
import { registerListPropertyValuesTool } from './tools/list-property-values.js';
import { registerUpdatePropertiesTool } from './tools/update-properties.js';
import { registerReadNotesTool } from './tools/read-notes.js';
import { registerReadNotePartTool } from './tools/read-note-part.js';
import { registerFileInfoTool } from './tools/file-info.js';
import { registerCreateFolderTool } from './tools/create-folder.js';
import { registerDeleteFolderTool } from './tools/delete-folder.js';
import { registerListAttachmentsTool } from './tools/list-attachments.js';
import { registerCreateAttachmentTool } from './tools/create-attachment.js';
import { registerListTasksTool } from './tools/list-tasks.js';
import { registerSetTaskStatusTool } from './tools/set-task-status.js';
import { registerEnsureBlockIdTool } from './tools/ensure-block-id.js';
import { registerFindBrokenLinksTool } from './tools/find-broken-links.js';
import { registerFindOrphanedNotesTool } from './tools/find-orphaned-notes.js';
import { registerSearchAndReplaceTool } from './tools/search-and-replace.js';
import { registerRenameTagTool } from './tools/rename-tag.js';
import { registerRenameHeadingTool } from './tools/rename-heading.js';

export interface ServerOptions {
  vaults: VaultSpec[];
  /** Each vault keeps its index in `<dataDir>/<name>`. */
  dataDir: string;
}

/**
 * Serves every vault over one stdio connection. Each tool takes a required
 * `vault` argument naming one of `opts.vaults`.
 */
export async function startServer(opts: ServerOptions): Promise<void> {
  debugLog('startServer: entry — registering signal handlers BEFORE any await');

  // v1.7.22 race fix: arm SIGINT/SIGTERM handlers FIRST, before any await.
  // The handler captures `vaults` via the `let` binding below, which is
  // empty until the contexts come online. Closing an empty list is correct —
  // there's nothing to clean up.
  //
  // Why this matters: createContext() dlopens better-sqlite3 + sqlite-vec
  // and opens the DB. On a cold Linux CI runner that takes ~1s. If the
  // host sends SIGTERM during that window, the kernel signal-kills with
  // code: null because Node hasn't registered a handler yet. Pre-v1.7.22
  // this raced silently green; the v1.7.22 imports (logger.ts, preparing.ts)
  // added enough cold-import overhead to flip the race red on CI.
  let vaults: Vault[] = [];
  let shuttingDown = false;
  const shutdown = async (reason: string): Promise<void> => {
    debugLog(`shutdown: invoked with reason="${reason}", shuttingDown=${shuttingDown}`);
    if (shuttingDown) return;
    shuttingDown = true;
    logger.info(`shutting down (${reason}).`, { reason });
    await closeVaults(vaults, closeContext);
    process.exitCode = 0;
    setTimeout(() => process.exit(0), 4_000).unref();
  };
  process.on('SIGINT', () => void shutdown('SIGINT'));
  process.on('SIGTERM', () => void shutdown('SIGTERM'));
  debugLog('startServer: SIGINT/SIGTERM handlers armed pre-init');

  debugLog('startServer: opening vaults');
  vaults = await openVaults(opts.vaults, opts.dataDir, closeContext);
  debugLog('startServer: vaults open, instantiating McpServer');
  const server = new McpServer({ name: 'obsidian-brain', version: pkg.version });
  new VaultTools(vaults, registerTools).register(server);
  debugLog('startServer: tools registered, instantiating StdioServerTransport');

  // Connect to the MCP transport immediately so the initialize handshake
  // completes in <100ms regardless of whether the embedding model has been
  // downloaded. Model download + first-time index proceed in the background.
  const transport = new StdioServerTransport();
  debugLog('startServer: calling server.connect(transport) — handshake will fire next');
  await server.connect(transport);
  debugLog('startServer: server.connect returned — transport is live, scheduling background block');

  // Background init: embedder download, bootstrap, and initial index all run
  // asynchronously after the handshake. ctx.embedderReady() exposes the state
  // to tool handlers; ctx.initError captures any failure for tools to surface.
  void indexVaultsInTurn(vaults, runStartupIndex, () => shuttingDown);

  // Live reindex on vault changes. Set OBSIDIAN_BRAIN_NO_WATCH=1 to fall
  // back to the timer-driven model (periodic `obsidian-brain index` runs).
  startVaultWatchers(vaults, readWatcherOptsFromEnv());
  debugLog('startServer: watchers started');

  // Session-end: the MCP SDK fires `transport.onclose` when the client ends
  // the JSON-RPC session cleanly. Wire our shutdown to it so we don't linger
  // after a normal disconnect.
  transport.onclose = () => void shutdown('MCP transport closed');

  // Orphan watcher: if the host process (Claude Desktop, Jan, Codex, Cursor,
  // VS Code) crashes or is force-quit without sending SIGTERM, this process
  // would otherwise keep running forever under launchd / init. We probe the
  // original parent PID once a minute with signal 0 (pure existence check,
  // no side effect). Works cross-platform — on macOS/Linux the OS reparents
  // us to PID 1 so the dead original PID trips the check; on Windows PPID
  // doesn't change on orphaning but signal 0 still throws ESRCH when the
  // original parent is gone. `.unref()` keeps the interval from pinning the
  // event loop when nothing else is alive.
  //
  // Previously we listened on `process.stdin` `end` / `close`, but that
  // false-fires under Jan: Jan closes stdin briefly between initialize and
  // the first tools/list while loading its local LLM, which would trigger
  // an immediate self-exit here.
  const originalPpid = process.ppid;
  setInterval(() => {
    try {
      process.kill(originalPpid, 0);
    } catch {
      void shutdown('parent process died (orphaned)');
    }
  }, 60_000).unref();
  debugLog(`startServer: orphan-PPID watchdog armed (parent PID=${originalPpid})`);
  debugLog('startServer: all wiring complete, function returning — server is now live');
}

/** Registers every tool on `server`, bound to one vault's context. */
export function registerTools(server: McpServer, ctx: ServerContext): void {
  registerSearchTool(server, ctx);
  registerReadNoteTool(server, ctx);
  registerListNotesTool(server, ctx);
  registerFindNotesByNameTool(server, ctx);
  registerGrepVaultTool(server, ctx);
  registerQueryNotesTool(server, ctx);
  registerFindConnectionsTool(server, ctx);
  registerFindPathBetweenTool(server, ctx);
  registerDetectThemesTool(server, ctx);
  registerRankNotesTool(server, ctx);
  registerCreateNoteTool(server, ctx);
  registerCreateNoteFromTemplateTool(server, ctx);
  registerEditNoteTool(server, ctx);
  registerApplyEditPreviewTool(server, ctx);
  registerLinkNotesTool(server, ctx);
  registerMoveNoteTool(server, ctx);
  registerDeleteNoteTool(server, ctx);
  registerReadCanvasTool(server, ctx);
  registerEditCanvasTool(server, ctx);
  registerReindexTool(server, ctx);
  registerActiveNoteTool(server, ctx);
  registerDataviewQueryTool(server, ctx);
  registerBaseQueryTool(server, ctx);
  registerIndexStatusTool(server, ctx);
  registerVaultOverviewTool(server, ctx);
  registerListTagsTool(server, ctx);
  registerListBookmarksTool(server, ctx);
  registerListPropertyValuesTool(server, ctx);
  registerUpdatePropertiesTool(server, ctx);
  registerReadNotesTool(server, ctx);
  registerReadNotePartTool(server, ctx);
  registerFileInfoTool(server, ctx);
  registerCreateFolderTool(server, ctx);
  registerDeleteFolderTool(server, ctx);
  registerListAttachmentsTool(server, ctx);
  registerCreateAttachmentTool(server, ctx);
  registerListTasksTool(server, ctx);
  registerSetTaskStatusTool(server, ctx);
  registerEnsureBlockIdTool(server, ctx);
  registerFindBrokenLinksTool(server, ctx);
  registerFindOrphanedNotesTool(server, ctx);
  registerSearchAndReplaceTool(server, ctx);
  registerRenameTagTool(server, ctx);
  registerRenameHeadingTool(server, ctx);
}

/** Waits briefly for queued indexing, then releases the embedder and the DB. */
export async function closeContext(ctx: ServerContext): Promise<void> {
  debugLog('shutdown: awaiting pendingReindex (max 3s)');
  await Promise.race([
    ctx.pendingReindex.catch(() => {}),
    new Promise<void>((resolve) => {
      setTimeout(resolve, 3_000).unref();
    }),
  ]);
  debugLog('shutdown: pendingReindex drained or timed out');
  if (ctx.embedderReady() && ctx.ownsEmbedder !== false) {
    debugLog('shutdown: disposing embedder (ONNX runtime threads)');
    await ctx.embedder.dispose();
    debugLog('shutdown: embedder disposed');
  }
  debugLog('shutdown: checkpointing WAL');
  try {
    ctx.db.exec('PRAGMA wal_checkpoint(TRUNCATE)');
    debugLog('shutdown: WAL checkpoint OK');
  } catch (err) {
    logger.warn(`WAL checkpoint failed during shutdown (ignored): ${err}`, {
      error: String(err),
    });
  }
  debugLog('shutdown: closing DB');
  ctx.db.close();
  debugLog('shutdown: DB closed');
}

/**
 * Background init: embedder download, bootstrap, and initial or catchup
 * index. Resolves once the index work is queued; `ctx.pendingReindex`
 * settles when it finishes. ctx.embedderReady() exposes the state to tool
 * handlers; ctx.initError captures any failure for tools to surface.
 */
export async function runStartupIndex(ctx: ServerContext, dbIsEmpty: boolean): Promise<void> {
  debugLog('background: entered fire-and-forget init block');
  try {
    if (dbIsEmpty) {
      debugLog('background: dbIsEmpty branch — first-boot, calling ensureEmbedderReady');
      // First-ever boot: download model and build initial index. Time
      // varies dramatically with vault size — small vaults complete in
      // under a minute; 10k+-note vaults take 5-15 minutes. Both factors
      // are unknown at this exact point (model not yet downloaded, vault
      // not yet walked), so the message is deliberately vague rather
      // than mis-promising "30-60s" like earlier releases.
      logger.info(
        'index is empty, running first-time index. ' +
          'Time depends on vault size — typically under a minute for small vaults, ' +
          'a few minutes for thousands of notes. Downloads embedding model on first boot.',
        { firstTime: true },
      );
      await ctx.ensureEmbedderReady();
      ctx.enqueueBackgroundReindex(async () => {
        const stats = await ctx.pipeline.index(ctx.config.vaultPath);
        logger.info(
          `indexed ${stats.nodesIndexed} notes, ` +
            `${stats.edgesIndexed} links, ${stats.communitiesDetected} communities.`,
          {
            nodesIndexed: stats.nodesIndexed,
            edgesIndexed: stats.edgesIndexed,
            communitiesDetected: stats.communitiesDetected,
          },
        );
      });
    } else {
      // Non-empty DB: surface any bootstrap migration reasons (model change,
      // v1.4.0 chunk upgrade, FTS tokenizer swap) so users understand why a
      // reindex kicks in. The actual reindex is handled by the catchup path
      // below — forcing all sync mtimes to 0 so every note re-embeds under
      // the new model.
      debugLog('background: non-empty DB branch — calling ensureEmbedderReady');
      await ctx.ensureEmbedderReady();
      debugLog('background: ensureEmbedderReady complete, calling getBootstrap');
      const boot = ctx.getBootstrap();
      if (boot) {
        for (const reason of boot.reasons) {
          logger.info(reason, { bootstrapReason: true });
        }
        if (boot.needsReindex) {
          // Force a from-scratch reindex by clearing sync mtimes — the indexer's
          // mtime-guard would otherwise skip every file.
          ctx.db.exec('DELETE FROM sync');
          logger.info('rebuilding per-chunk embeddings (may take a minute)...', {
            rebuildAll: true,
          });
        }
      }

      // Subsequent-boot catchup: the client has gone away and come back, and
      // notes may have been edited on disk in the meantime. Run an incremental
      // full-vault reindex in the background so the client gets `tools/list`
      // immediately; the watcher takes over for any live edits from here on.
      // Set OBSIDIAN_BRAIN_NO_CATCHUP=1 to disable.
      const wasReindexFromScratch = boot?.needsReindex ?? false;
      if (process.env.OBSIDIAN_BRAIN_NO_CATCHUP !== '1') {
        ctx.enqueueBackgroundReindex(async () => {
          const stats = await ctx.pipeline.index(ctx.config.vaultPath);
          if (stats.nodesIndexed > 0) {
            const suffix = wasReindexFromScratch
              ? 're-embedded after model/schema change'
              : 'modified while the server was down';
            logger.info(
              `startup catchup — reindexed ${stats.nodesIndexed} note(s) (${suffix})`,
              {
                nodesIndexed: stats.nodesIndexed,
                reason: wasReindexFromScratch ? 'model-or-schema-change' : 'edits-while-down',
              },
            );
          }
        });
      }
    }
    debugLog('background: init block completed without errors');
  } catch (err) {
    ctx.initError = err;
    logger.error(`background init failed: ${err}`, { error: String(err) });
    debugLog(`background: init block CAUGHT error — ${err instanceof Error ? err.message : String(err)}`);
  }
}

export function readWatcherOptsFromEnv() {
  const debounceMs = Number(process.env.OBSIDIAN_BRAIN_WATCH_DEBOUNCE_MS);
  const communityDebounceMs = Number(
    process.env.OBSIDIAN_BRAIN_COMMUNITY_DEBOUNCE_MS,
  );
  return {
    debounceMs: Number.isFinite(debounceMs) && debounceMs > 0 ? debounceMs : undefined,
    communityDebounceMs:
      Number.isFinite(communityDebounceMs) && communityDebounceMs > 0
        ? communityDebounceMs
        : undefined,
  };
}
