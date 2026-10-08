import { afterEach, describe, expect, it } from 'vitest';
import { chmod } from 'node:fs/promises';
import { join } from 'node:path';
import { expandTemplate, registerSearchAndReplaceTool } from '../../src/tools/search-and-replace.js';
import { makeMockServer, unwrap } from '../helpers/mock-server.js';
import { makeVault, type VaultFixture } from '../helpers/vault-fixture.js';

let fx: VaultFixture | undefined;
afterEach(async () => {
  await fx?.cleanup();
  fx = undefined;
});

// eslint-disable-next-line @typescript-eslint/no-explicit-any
async function setup(files: Record<string, string>): Promise<(args: Record<string, unknown>) => Promise<any>> {
  fx = await makeVault(files);
  const { server, registered } = makeMockServer();
  registerSearchAndReplaceTool(server, fx.ctx);
  return (args) => registered[0]!.cb(args);
}

describe('search_and_replace', () => {
  it('dry run is the default: counts and samples, no writes, no reindex', async () => {
    const call = await setup({ 'A.md': 'cat and cat\nno match\nthe cat\n', 'B.md': 'dog' });
    const out = unwrap(await call({ pattern: 'cat', replacement: 'dog' }));
    expect(out).toEqual({
      dryRun: true,
      filesMatched: 1,
      totalMatches: 3,
      files: [
        {
          path: 'A.md',
          matches: 3,
          samples: [
            { line: 1, before: 'cat and cat', after: 'dog and dog' },
            { line: 3, before: 'the cat', after: 'the dog' },
          ],
        },
      ],
    });
    expect(await fx!.read('A.md')).toBe('cat and cat\nno match\nthe cat\n');
    expect(fx!.ctx.reindexCalls).toBe(0);
  });

  it('writes with dryRun false and reindexes once', async () => {
    const call = await setup({ 'A.md': 'a.b a.b', 'B.md': 'xa.by', 'C.md': 'none' });
    const out = unwrap(await call({ pattern: 'a.b', replacement: '$1', dryRun: false }));
    expect(out).toMatchObject({ dryRun: false, filesChanged: 2, totalMatches: 3 });
    expect(await fx!.read('A.md')).toBe('$1 $1');
    expect(await fx!.read('B.md')).toBe('x$1y');
    expect(fx!.ctx.reindexCalls).toBe(1);
  });

  it('regex mode expands groups; caseSensitive false ignores case', async () => {
    const call = await setup({ 'A.md': 'Foo-1 foo-2\n' });
    unwrap(
      await call({ pattern: '(f)oo-(?<n>\\d)', replacement: '$2|$<n>|$1|$&|$$', regex: true, caseSensitive: false, dryRun: false }),
    );
    expect(await fx!.read('A.md')).toBe('1|1|F|Foo-1|$ 2|2|f|foo-2|$\n');
  });

  it('is case-sensitive by default and anchors ^ per line', async () => {
    const call = await setup({ 'A.md': 'Cat\ncat\n' });
    unwrap(await call({ pattern: '^cat', replacement: 'dog', regex: true, dryRun: false }));
    expect(await fx!.read('A.md')).toBe('Cat\ndog\n');
  });

  it('leaves frontmatter and fenced code alone unless asked', async () => {
    const raw = '---\ntitle: old\n---\nold\n```\nold\n```\n`old`\n';
    const call = await setup({ 'A.md': raw });
    unwrap(await call({ pattern: 'old', replacement: 'new', dryRun: false }));
    expect(await fx!.read('A.md')).toBe('---\ntitle: old\n---\nnew\n```\nold\n```\n`new`\n');

    unwrap(await call({ pattern: 'old', replacement: 'new', includeFrontmatter: true, includeCode: true, dryRun: false }));
    expect(await fx!.read('A.md')).toBe('---\ntitle: new\n---\nnew\n```\nnew\n```\n`new`\n');
  });

  it('a match never spans into a code fence', async () => {
    const call = await setup({ 'A.md': 'x\n```\ny\n```\n' });
    const out = unwrap(await call({ pattern: 'x\\s+```', replacement: '', regex: true }));
    expect(out.filesMatched).toBe(0);
  });

  it('restricts to a folder', async () => {
    const call = await setup({ 'a/A.md': 'hit', 'b/B.md': 'hit' });
    const out = unwrap(await call({ pattern: 'hit', replacement: 'x', folder: 'a' }));
    expect(out.files.map((f: { path: string }) => f.path)).toEqual(['a/A.md']);
  });

  it('maxFiles truncates a dry run and refuses a write without touching files', async () => {
    const call = await setup({ 'A.md': 'hit', 'B.md': 'hit', 'C.md': 'hit' });
    const dry = unwrap(await call({ pattern: 'hit', replacement: 'x', maxFiles: 2 }));
    expect(dry).toMatchObject({ filesMatched: 3, truncated: true });
    expect(dry.files).toHaveLength(2);

    const res = await call({ pattern: 'hit', replacement: 'x', maxFiles: 2, dryRun: false });
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toMatch(/3 files match, more than maxFiles \(2\)/);
    expect(await fx!.read('A.md')).toBe('hit');
  });

  it('rejects unsafe, invalid and over-long patterns', async () => {
    const call = await setup({ 'A.md': 'aaa' });
    for (const [pattern, msg] of [
      ['(a+)+', /quantified group/],
      ['[', /Invalid regex/],
      ['a'.repeat(501), /too long/],
    ] as const) {
      const res = await call({ pattern, replacement: 'x', regex: true });
      expect(res.isError).toBe(true);
      expect(res.content[0].text).toMatch(msg);
    }
    expect(await fx!.read('A.md')).toBe('aaa');
  });

  it('samples center on the match in a long line and group matches per line', async () => {
    const long = `${'x'.repeat(200)} hit ${'y'.repeat(200)}`;
    const call = await setup({ 'A.md': `${long}\nhit\nhit\nhit\nhit` });
    const out = unwrap(await call({ pattern: 'hit', replacement: 'HIT' }));
    const [first, ...rest] = out.files[0].samples;
    expect(first.before.length).toBeLessThan(200);
    expect(first.after).toContain(' HIT ');
    expect(rest.map((s: { line: number }) => s.line)).toEqual([2, 3]);
  });

  it('reports a failed write and keeps going', async () => {
    const call = await setup({ 'a/A.md': 'hit', 'B.md': 'hit' });
    await chmod(join(fx!.vault, 'a'), 0o500);
    try {
      const out = unwrap(await call({ pattern: 'hit', replacement: 'x', dryRun: false }));
      expect(out.filesChanged).toBe(1);
      expect(out.failed).toEqual([{ path: 'a/A.md', error: expect.any(String) }]);
      expect(await fx!.read('B.md')).toBe('x');
    } finally {
      await chmod(join(fx!.vault, 'a'), 0o700);
    }
  });
});

describe('expandTemplate', () => {
  const m = /(a)(b)?/.exec('xaby')!;
  it('follows String.prototype.replace rules', () => {
    expect(expandTemplate("$`|$'|$3|$12|$0|$<n>", m, 'xaby')).toBe("x|y|$3|a2|$0|$<n>");
    const n = /(?<n>a)/.exec('a')!;
    expect(expandTemplate('$<n>$<zz>', n, 'a')).toBe('a');
  });
});
