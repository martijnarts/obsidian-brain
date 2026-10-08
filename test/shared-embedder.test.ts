/**
 * shareEmbedder lets several vaults' contexts use one embedder, whose model
 * loads once however many contexts ask for it.
 */

import { describe, expect, it, vi } from 'vitest';
import { shareEmbedder } from '../src/context.js';
import type { Embedder } from '../src/embeddings/types.js';

function countingEmbedder(init: () => Promise<void>): Embedder {
  return { init: vi.fn(init) } as unknown as Embedder;
}

describe('shareEmbedder', () => {
  it('loads the model once for every caller, also concurrent ones', async () => {
    const embedder = countingEmbedder(async () => {});
    const shared = shareEmbedder(embedder);
    await Promise.all([shared.init(), shared.init(), shared.init()]);
    await shared.init();
    expect(embedder.init).toHaveBeenCalledTimes(1);
    expect(shared.embedder).toBe(embedder);
  });

  it('gives every caller the same failure', async () => {
    const embedder = countingEmbedder(async () => {
      throw new Error('model unavailable');
    });
    const shared = shareEmbedder(embedder);
    await expect(shared.init()).rejects.toThrow('model unavailable');
    await expect(shared.init()).rejects.toThrow('model unavailable');
    expect(embedder.init).toHaveBeenCalledTimes(1);
  });
});
