import { relative } from 'path';
import chokidar, { type FSWatcher } from 'chokidar';
import type { ServerContext } from '../context.js';
import { debugLog } from '../util/debug-log.js';
import { logger } from '../util/logger.js';

debugLog('module-load: src/pipeline/watcher.ts');

export interface WatcherOptions {
  /** Per-file reindex debounce (ms). Collapses bursts of writes from
   *  Obsidian's autosave into a single reindex. */
  debounceMs?: number;
}

export interface WatcherHandle {
  /** Stop watching and release all resources. */
  close: () => Promise<void>;
  /** Underlying chokidar watcher (exposed for tests + advanced callers). */
  watcher: FSWatcher;
}

const DEFAULT_DEBOUNCE_MS = 3_000;

/**
 * Watch the vault and keep the index live. Chokidar's awaitWriteFinish +
 * our own per-file debounce collapses Obsidian's ~2s autosave cadence into
 * a single reindex per editing pause.
 */
export function startWatcher(
  ctx: ServerContext,
  opts: WatcherOptions = {},
): WatcherHandle {
  const vaultPath = ctx.config.vaultPath;
  const debounceMs = opts.debounceMs ?? DEFAULT_DEBOUNCE_MS;

  const pendingFiles = new Map<string, NodeJS.Timeout>();
  const inFlight = new Set<Promise<unknown>>();
  let shuttingDown = false;

  const track = <T>(p: Promise<T>): Promise<T> => {
    inFlight.add(p);
    p.finally(() => inFlight.delete(p));
    return p;
  };

  const watcher = chokidar.watch(vaultPath, {
    ignored: (path: string) => {
      if (/(^|\/)(\.obsidian|\.trash|\.git|node_modules|attachments)(\/|$)/.test(path)) {
        return true;
      }
      // Allow directories through; only filter non-md files.
      if (/\.[A-Za-z0-9]+$/.test(path) && !/\.md$/i.test(path)) {
        return true;
      }
      return false;
    },
    ignoreInitial: true,
    awaitWriteFinish: { stabilityThreshold: 2_000, pollInterval: 150 },
    persistent: true,
  });

  const scheduleFile = (
    absPath: string,
    event: 'add' | 'change' | 'unlink',
  ) => {
    const relPath = relative(vaultPath, absPath);
    if (!relPath.endsWith('.md') || relPath.startsWith('..')) return;

    const existing = pendingFiles.get(relPath);
    if (existing) clearTimeout(existing);

    pendingFiles.set(
      relPath,
      setTimeout(() => {
        pendingFiles.delete(relPath);
        if (shuttingDown) return;
        track(
          (async () => {
            try {
              await ctx.ensureEmbedderReady();
              const result = await ctx.pipeline.indexSingleNote(
                vaultPath,
                relPath,
                event,
              );
              if (result.indexed || result.deleted) {
                const verb = result.deleted ? 'removed' : event;
                logger.info(
                  `${verb} ${relPath}` +
                    (result.stubsCreated > 0 ? ` (+${result.stubsCreated} stubs)` : ''),
                  { event: verb, path: relPath, stubsCreated: result.stubsCreated },
                );
              }
            } catch (err) {
              const errMsg = err instanceof Error ? err.message : String(err);
              logger.error(`reindex failed for ${relPath}: ${errMsg}`, {
                path: relPath,
                error: errMsg,
              });
            }
          })(),
        );
      }, debounceMs),
    );
  };

  watcher.on('add', (p) => scheduleFile(p, 'add'));
  watcher.on('change', (p) => scheduleFile(p, 'change'));
  watcher.on('unlink', (p) => scheduleFile(p, 'unlink'));
  watcher.on('error', (err) => {
    const errMsg = err instanceof Error ? err.message : String(err);
    logger.error(`watcher error: ${errMsg}`, { error: errMsg });
  });

  logger.info(`watching ${vaultPath} for changes`, { vaultPath });

  const close = async () => {
    shuttingDown = true;
    for (const timer of pendingFiles.values()) clearTimeout(timer);
    pendingFiles.clear();
    await watcher.close();
    // Drain in-flight work so the DB isn't closed mid-operation by the caller.
    await Promise.allSettled([...inFlight]);
  };

  return { close, watcher };
}
