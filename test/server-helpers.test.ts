/**
 * Unit tests for the per-vault helpers that `startServer` (stdio) and
 * `startHttpServer` share: tool registration, the background startup index,
 * and context teardown.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { ServerContext } from '../src/context.js';
import { closeContext, registerTools, runStartupIndex } from '../src/server.js';

interface FakeCtx {
  ctx: ServerContext;
  index: ReturnType<typeof vi.fn>;
  exec: ReturnType<typeof vi.fn>;
  dispose: ReturnType<typeof vi.fn>;
  close: ReturnType<typeof vi.fn>;
}

function fakeCtx(
  opts: {
    ensure?: () => Promise<void>;
    boot?: { needsReindex: boolean; reasons: string[] } | null;
    nodesIndexed?: number;
    embedderReady?: boolean;
    execThrows?: boolean;
  } = {},
): FakeCtx {
  const index = vi.fn(async () => ({
    nodesIndexed: opts.nodesIndexed ?? 0,
    edgesIndexed: 0,
    communitiesDetected: 0,
  }));
  const exec = vi.fn(() => {
    if (opts.execThrows) throw new Error('checkpoint failed');
  });
  const dispose = vi.fn(async () => {});
  const close = vi.fn();
  const ctx = {
    db: { exec, close },
    embedder: { dispose },
    config: { vaultPath: '/vault' },
    pipeline: { index },
    ensureEmbedderReady: opts.ensure ?? (async () => {}),
    getBootstrap: () => opts.boot ?? null,
    embedderReady: () => opts.embedderReady ?? false,
    initError: undefined,
    pendingReindex: Promise.resolve(),
    enqueueBackgroundReindex(work: () => Promise<void>) {
      ctx.pendingReindex = ctx.pendingReindex.then(work);
    },
  } as unknown as ServerContext;
  return { ctx, index, exec, dispose, close };
}

describe('registerTools', () => {
  it('registers all 23 tools', () => {
    const names: string[] = [];
    const server = {
      tool: (name: string) => names.push(name),
      registerTool: (name: string) => names.push(name),
    };
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    registerTools(server as any, fakeCtx().ctx);
    expect(new Set(names).size).toBe(23);
    expect(names).toEqual(expect.arrayContaining(['search', 'edit_note', 'index_status']));
  });
});

describe('runStartupIndex', () => {
  beforeEach(() => {
    vi.stubEnv('OBSIDIAN_BRAIN_NO_CATCHUP', '');
  });

  afterEach(() => {
    vi.unstubAllEnvs();
  });

  it('builds the first index of an empty DB', async () => {
    const f = fakeCtx({ nodesIndexed: 3 });
    await runStartupIndex(f.ctx, true);
    await f.ctx.pendingReindex;
    expect(f.index).toHaveBeenCalledWith('/vault');
    expect(f.exec).not.toHaveBeenCalled();
  });

  it('catches up a non-empty DB incrementally', async () => {
    const f = fakeCtx({ boot: { needsReindex: false, reasons: [] }, nodesIndexed: 2 });
    await runStartupIndex(f.ctx, false);
    await f.ctx.pendingReindex;
    expect(f.index).toHaveBeenCalledWith('/vault');
    expect(f.exec).not.toHaveBeenCalled();
  });

  it('clears sync state first when the bootstrap asks for a full reindex', async () => {
    const f = fakeCtx({ boot: { needsReindex: true, reasons: ['model changed'] }, nodesIndexed: 5 });
    await runStartupIndex(f.ctx, false);
    await f.ctx.pendingReindex;
    expect(f.exec).toHaveBeenCalledWith('DELETE FROM sync');
    expect(f.index).toHaveBeenCalledWith('/vault');
  });

  it('skips the catchup when OBSIDIAN_BRAIN_NO_CATCHUP=1', async () => {
    vi.stubEnv('OBSIDIAN_BRAIN_NO_CATCHUP', '1');
    const f = fakeCtx();
    await runStartupIndex(f.ctx, false);
    await f.ctx.pendingReindex;
    expect(f.index).not.toHaveBeenCalled();
  });

  it('records an embedder failure on the context instead of throwing', async () => {
    const failure = new Error('embedder unavailable');
    const f = fakeCtx({ ensure: () => Promise.reject(failure) });
    await expect(runStartupIndex(f.ctx, true)).resolves.toBeUndefined();
    expect(f.ctx.initError).toBe(failure);
    expect(f.index).not.toHaveBeenCalled();
  });
});

describe('closeContext', () => {
  it('disposes a ready embedder, checkpoints the WAL and closes the DB', async () => {
    const f = fakeCtx({ embedderReady: true });
    await closeContext(f.ctx);
    expect(f.dispose).toHaveBeenCalled();
    expect(f.exec).toHaveBeenCalledWith('PRAGMA wal_checkpoint(TRUNCATE)');
    expect(f.close).toHaveBeenCalled();
  });

  it('leaves a shared embedder to whoever shared it', async () => {
    const f = fakeCtx({ embedderReady: true });
    Object.assign(f.ctx, { ownsEmbedder: false });
    await closeContext(f.ctx);
    expect(f.dispose).not.toHaveBeenCalled();
    expect(f.close).toHaveBeenCalled();
  });

  it('leaves an embedder that never loaded alone', async () => {
    const f = fakeCtx({ embedderReady: false });
    await closeContext(f.ctx);
    expect(f.dispose).not.toHaveBeenCalled();
    expect(f.close).toHaveBeenCalled();
  });

  it('still closes the DB when the WAL checkpoint fails', async () => {
    const f = fakeCtx({ execThrows: true });
    await closeContext(f.ctx);
    expect(f.close).toHaveBeenCalled();
  });

  it('stops waiting for queued indexing after 3 seconds', async () => {
    vi.useFakeTimers();
    try {
      const f = fakeCtx();
      f.ctx.pendingReindex = new Promise(() => {});
      const closing = closeContext(f.ctx);
      await vi.advanceTimersByTimeAsync(3_000);
      await closing;
      expect(f.close).toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });
});
