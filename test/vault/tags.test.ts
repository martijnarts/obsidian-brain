import { describe, it, expect } from 'vitest';
import { noteTags, tagMatches, tagWithParents, countTags, sortTagCounts } from '../../src/vault/tags.js';

describe('vault/tags', () => {
  it('noteTags merges tags, tag and inline_tags, strips # and dedupes', () => {
    expect(
      noteTags({ tags: ['#a', 'b/c'], tag: 'a', inline_tags: ['d', 'b/c'] }).sort(),
    ).toEqual(['a', 'b/c', 'd']);
  });

  it('noteTags splits a string value on commas and whitespace', () => {
    expect(noteTags({ tags: '#x, y z' }).sort()).toEqual(['x', 'y', 'z']);
  });

  it('noteTags ignores non-string values and empty entries', () => {
    expect(noteTags({ tags: [1, null, '', '#'] })).toEqual([]);
    expect(noteTags({})).toEqual([]);
  });

  it('tagMatches accepts the tag itself and nested tags only', () => {
    expect(tagMatches('a', 'a')).toBe(true);
    expect(tagMatches('a/b', 'a')).toBe(true);
    expect(tagMatches('a/b', '#a')).toBe(true);
    expect(tagMatches('ab', 'a')).toBe(false);
    expect(tagMatches('a', 'a/b')).toBe(false);
  });

  it('tagWithParents expands every ancestor', () => {
    expect(tagWithParents('a/b/c')).toEqual(['a', 'a/b', 'a/b/c']);
    expect(tagWithParents('a')).toEqual(['a']);
  });

  it('countTags counts each note once per tag, with or without parents', () => {
    const fms = [{ tags: ['a/b', 'a/c'] }, { tags: ['a'] }];
    expect(Object.fromEntries(countTags(fms, true))).toEqual({ a: 2, 'a/b': 1, 'a/c': 1 });
    expect(Object.fromEntries(countTags(fms, false))).toEqual({ a: 1, 'a/b': 1, 'a/c': 1 });
  });

  it('sortTagCounts orders by count desc then name', () => {
    const counts = new Map([['b', 1], ['a', 1], ['c', 2]]);
    expect(sortTagCounts(counts).map((t) => t.tag)).toEqual(['c', 'a', 'b']);
  });
});
