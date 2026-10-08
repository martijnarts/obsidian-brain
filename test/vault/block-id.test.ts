import { describe, it, expect } from 'vitest';
import {
  allBlockIds,
  attachBlockId,
  existingBlockId,
  findHeadingLines,
  generateBlockId,
  locateBlock,
} from '../../src/vault/block-id.js';

describe('block-id', () => {
  it('bounds a paragraph by blank lines and other blocks', () => {
    const lines = ['# H', 'one', 'two', '- item', 'three'];
    expect(locateBlock(lines, 1)).toEqual({ kind: 'inline', start: 1, end: 2 });
    expect(locateBlock(lines, 4)).toEqual({ kind: 'inline', start: 4, end: 4 });
  });

  it('reads an own-line id directly after the block or after one blank line', () => {
    const block = { kind: 'own-line' as const, start: 0, end: 0 };
    expect(existingBlockId(['> q', '^a'], block)).toEqual({ id: 'a', line: 1 });
    expect(existingBlockId(['> q', '', '^b'], block)).toEqual({ id: 'b', line: 2 });
    expect(existingBlockId(['> q', '', 'text'], block)).toBeNull();
    expect(existingBlockId(['> q'], block)).toBeNull();
  });

  it('does not add a blank line when one already follows', () => {
    const block = { kind: 'own-line' as const, start: 0, end: 0 };
    expect(attachBlockId(['| a |', '', 'next'], block, 'z').lines).toEqual(['| a |', '', '^z', '', 'next']);
  });

  it('collects trailing and own-line ids, and generates fresh ones', () => {
    const taken = allBlockIds(['text ^one', '^two', 'a^not']);
    expect([...taken]).toEqual(['one', 'two']);
    expect(taken.has(generateBlockId(taken))).toBe(false);
  });

  it('matches headings outside fences, ignoring closing hashes and ids', () => {
    const lines = ['## Plan ##', '```', '## Plan', '```', '### Plan ^p', '#Plan'];
    expect(findHeadingLines(lines, 'Plan')).toEqual([0, 4]);
  });
});
