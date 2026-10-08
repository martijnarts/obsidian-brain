/**
 * TransformersEmbedder unloads its model after an idle period and loads it
 * again on the next embed. The pipeline is mocked: each load returns a fresh
 * extractor, and the test counts loads and disposes.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

interface FakeExtractor {
  (text: string): Promise<{ tolist(): number[][] }>;
  dispose: ReturnType<typeof vi.fn>;
  tokenizer: { model_max_length: number };
}

let loads = 0;
let extractors: FakeExtractor[] = [];
/** While set, every embed waits for it to resolve. */
let gate: Promise<void> | null = null;

vi.mock('@huggingface/transformers', () => ({
  env: {},
  pipeline: async () => {
    loads++;
    const extractor = (async (text: string) => {
      if (gate) await gate;
      return { tolist: () => [[text.length, 1, 2]] };
    }) as FakeExtractor;
    extractor.dispose = vi.fn(async () => {});
    extractor.tokenizer = { model_max_length: 512 };
    extractors.push(extractor);
    return extractor;
  },
}));

const { TransformersEmbedder } = await import('../../src/embeddings/embedder.js');

const IDLE_MS = 60_000;

describe('TransformersEmbedder idle unload', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    loads = 0;
    extractors = [];
    gate = null;
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllEnvs();
  });

  it('keeps the model loaded when no idle time is set', async () => {
    const embedder = new TransformersEmbedder('test/model', { idleUnloadMs: 0 });
    await embedder.init();
    await vi.advanceTimersByTimeAsync(24 * 60 * 60 * 1000);
    expect(embedder.isLoaded()).toBe(true);
    expect(extractors[0]!.dispose).not.toHaveBeenCalled();
  });

  it('unloads the model after the idle time and reloads it on the next embed', async () => {
    const embedder = new TransformersEmbedder('test/model', { idleUnloadMs: IDLE_MS });
    await embedder.init();
    expect(loads).toBe(1);

    await vi.advanceTimersByTimeAsync(IDLE_MS);
    expect(embedder.isLoaded()).toBe(false);
    expect(extractors[0]!.dispose).toHaveBeenCalledTimes(1);

    const vec = await embedder.embed('hello');
    expect(Array.from(vec)).toEqual([5, 1, 2]);
    expect(loads).toBe(2);
    expect(embedder.isLoaded()).toBe(true);
  });

  it('restarts the idle clock on every embed', async () => {
    const embedder = new TransformersEmbedder('test/model', { idleUnloadMs: IDLE_MS });
    await embedder.init();
    await vi.advanceTimersByTimeAsync(IDLE_MS - 1_000);
    await embedder.embed('keep me');
    await vi.advanceTimersByTimeAsync(IDLE_MS - 1_000);
    expect(embedder.isLoaded()).toBe(true);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(embedder.isLoaded()).toBe(false);
  });

  it('never unloads under an embed that is still running', async () => {
    const embedder = new TransformersEmbedder('test/model', { idleUnloadMs: IDLE_MS });
    await embedder.init();
    let release!: () => void;
    gate = new Promise((resolve) => (release = resolve));
    const slow = embedder.embed('slow');
    await vi.advanceTimersByTimeAsync(IDLE_MS * 3);
    expect(embedder.isLoaded()).toBe(true);

    release();
    await slow;
    gate = null;
    await vi.advanceTimersByTimeAsync(IDLE_MS);
    expect(embedder.isLoaded()).toBe(false);
    expect(loads).toBe(1);
  });

  it('still reports the tokenizer limit while unloaded', async () => {
    const embedder = new TransformersEmbedder('test/model', { idleUnloadMs: IDLE_MS });
    await embedder.init();
    await vi.advanceTimersByTimeAsync(IDLE_MS);
    expect(embedder.isLoaded()).toBe(false);
    expect(embedder.modelMaxLength).toBe(512);
    expect(embedder.dimensions()).toBe(3);
  });

  it('reads the idle time from OBSIDIAN_BRAIN_EMBEDDER_IDLE_MS', async () => {
    vi.stubEnv('OBSIDIAN_BRAIN_EMBEDDER_IDLE_MS', String(IDLE_MS));
    const embedder = new TransformersEmbedder('test/model');
    await embedder.init();
    await vi.advanceTimersByTimeAsync(IDLE_MS);
    expect(embedder.isLoaded()).toBe(false);
  });

  it.each([['0'], ['-5'], ['soon']])(
    'keeps the model loaded for OBSIDIAN_BRAIN_EMBEDDER_IDLE_MS=%s',
    async (value) => {
      vi.stubEnv('OBSIDIAN_BRAIN_EMBEDDER_IDLE_MS', value);
      const embedder = new TransformersEmbedder('test/model');
      await embedder.init();
      await vi.advanceTimersByTimeAsync(IDLE_MS * 10);
      expect(embedder.isLoaded()).toBe(true);
    },
  );

  it('dispose cancels a pending unload', async () => {
    const embedder = new TransformersEmbedder('test/model', { idleUnloadMs: IDLE_MS });
    await embedder.init();
    await embedder.dispose();
    expect(extractors[0]!.dispose).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(IDLE_MS);
    expect(extractors[0]!.dispose).toHaveBeenCalledTimes(1);
  });

  it('refuses to embed before init', async () => {
    const embedder = new TransformersEmbedder('test/model', { idleUnloadMs: IDLE_MS });
    await expect(embedder.embed('x')).rejects.toThrow(/not initialized/);
  });

  it('logs a failed unload and keeps embedding', async () => {
    const embedder = new TransformersEmbedder('test/model', { idleUnloadMs: IDLE_MS });
    await embedder.init();
    extractors[0]!.dispose.mockRejectedValueOnce(new Error('ort busy'));
    await vi.advanceTimersByTimeAsync(IDLE_MS);
    const vec = await embedder.embed('after');
    expect(vec.length).toBe(3);
    expect(loads).toBe(2);
  });
});
