import { describe, expect, it } from 'vitest';
import { findHeadings, findWikiLinks, maskCode, splitFrontmatter } from '../../src/vault/scan.js';

describe('scan helpers', () => {
  it('splitFrontmatter handles none, empty, CRLF and a fence at end of file', () => {
    expect(splitFrontmatter('body')).toEqual({ frontmatter: '', body: 'body' });
    expect(splitFrontmatter('---\n---\nx')).toEqual({ frontmatter: '---\n---\n', body: 'x' });
    expect(splitFrontmatter('---\r\na: 1\r\n---\r\nx').body).toBe('x');
    expect(splitFrontmatter('---\na: 1\n---')).toEqual({ frontmatter: '---\na: 1\n---', body: '' });
  });

  it('maskCode keeps length and newlines, blanks fences and inline spans', () => {
    const text = 'a `b` c\n```js\n[[x]]\n```\n~~~\nopen fence\n';
    const masked = maskCode(text);
    expect(masked).toHaveLength(text.length);
    expect(masked.split('\n')).toHaveLength(text.split('\n').length);
    expect(masked).not.toContain('b');
    expect(masked).not.toContain('[[x]]');
    expect(masked).not.toContain('open fence');
    expect(maskCode('x `y` z', true)).toBe('x `y` z');
    expect(maskCode('a ``b ` c`` d')).toBe('a           d');
  });

  it('findHeadings reads ATX headings with closing hashes, outside code', () => {
    const hs = findHeadings('# One\n#nope\n```\n# In code\n```\n### Three ###\n#  \n');
    expect(hs.map((h) => [h.level, h.text, h.line])).toEqual([
      [1, 'One', 1],
      [3, 'Three', 6],
    ]);
  });

  it('findWikiLinks splits target, subpath and alias', () => {
    const [a, b, c] = findWikiLinks('[[N#H|al]] ![[M^b]] [[#S]]');
    expect(a).toMatchObject({ embed: false, target: 'N', subpath: 'H', alias: 'al' });
    expect(b).toMatchObject({ embed: true, target: 'M', subpath: '^b', alias: null });
    expect(c).toMatchObject({ target: '', subpath: 'S' });
  });
});
