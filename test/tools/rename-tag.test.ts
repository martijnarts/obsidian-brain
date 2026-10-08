import { afterEach, describe, expect, it } from 'vitest';
import { chmod } from 'node:fs/promises';
import { join } from 'node:path';
import {
  isValidTag,
  normalizeTag,
  registerRenameTagTool,
  renameInFrontmatter,
  renameTagInNote,
} from '../../src/tools/rename-tag.js';
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
  registerRenameTagTool(server, fx.ctx);
  return (args) => registered[0]!.cb(args);
}

describe('rename_tag helpers', () => {
  it('normalizeTag and isValidTag', () => {
    expect(normalizeTag(' ##proj ')).toBe('proj');
    expect(isValidTag('proj/a-b_c')).toBe(true);
    expect(isValidTag('ünï')).toBe(true);
    for (const bad of ['', '123', '/a', 'a/', 'a//b', 'a b', 'a#b']) expect(isValidTag(bad)).toBe(false);
  });

  it('renames whole inline tags only, nested by default, case-insensitively', () => {
    const body = '#old #Old #older #old/child #old-x x#old a/#old\n#old at start';
    expect(renameTagInNote(body, 'old', 'new', true)).toEqual({
      text: '#new #new #older #new/child #old-x x#old a/#old\n#new at start',
      inline: 4,
      frontmatter: 0,
    });
    expect(renameTagInNote('#old/child #old', 'old', 'new', false).text).toBe('#old/child #new');
  });

  it('skips code, URLs and headings', () => {
    const body = '# old\n## old heading\n`#old`\n```\n#old\n```\nhttp://x.com/#old http://x#old\n[l](https://a.b/c#old)\n#old';
    const res = renameTagInNote(body, 'old', 'new', true);
    expect(res.inline).toBe(1);
    expect(res.text.endsWith('\n#new')).toBe(true);
    expect(res.text.slice(0, -4)).toBe(body.slice(0, -4));
  });

  it('renames frontmatter tag lists, strings and flow lists, keeping other bytes', () => {
    const fm = [
      '---',
      'title: old  # keep',
      'tags:',
      '  - old',
      '  - "#old/sub"',
      "  - 'older'",
      '',
      '  # comment',
      '  - Old',
      'aliases:',
      '  - old',
      'tag: [a, "#old", old]',
      'Tags: one, old two',
      '---',
      '',
    ].join('\n');
    const res = renameInFrontmatter(fm, 'old', 'new', true);
    expect(res.count).toBe(6);
    expect(res.text).toBe(
      fm
        .replace('  - old\n  - "#old/sub"', '  - new\n  - "#new/sub"')
        .replace('  - Old', '  - new')
        .replace('tag: [a, "#old", old]', 'tag: [a, "#new", new]')
        .replace('Tags: one, old two', 'Tags: one, new two'),
    );
    expect(renameInFrontmatter('', 'old', 'new', true)).toEqual({ text: '', count: 0 });
  });
});

describe('rename_tag', () => {
  it('dry run by default: per-file counts, no writes', async () => {
    const call = await setup({
      'A.md': '---\ntags: [old]\n---\n#old text',
      'B.md': '#other',
    });
    const out = unwrap(await call({ from: '#old', to: 'new' }));
    expect(out).toEqual({
      dryRun: true,
      from: 'old',
      to: 'new',
      filesMatched: 1,
      replacements: 2,
      files: [{ path: 'A.md', inline: 1, frontmatter: 1 }],
    });
    expect(await fx!.read('A.md')).toBe('---\ntags: [old]\n---\n#old text');
    expect(fx!.ctx.reindexCalls).toBe(0);
  });

  it('writes with dryRun false and reindexes once', async () => {
    const call = await setup({
      'A.md': '---\ntags: [old]\n---\n#old text #old/x',
      'B.md': '#old',
    });
    const out = unwrap(await call({ from: 'old', to: 'area/new', dryRun: false }));
    expect(out.replacements).toBe(4);
    expect(await fx!.read('A.md')).toBe('---\ntags: [area/new]\n---\n#area/new text #area/new/x');
    expect(await fx!.read('B.md')).toBe('#area/new');
    expect(fx!.ctx.reindexCalls).toBe(1);
  });

  it('includeNested false leaves child tags', async () => {
    const call = await setup({ 'A.md': '#old #old/x' });
    unwrap(await call({ from: 'old', to: 'new', includeNested: false, dryRun: false }));
    expect(await fx!.read('A.md')).toBe('#new #old/x');
  });

  it('rejects invalid and identical tags', async () => {
    const call = await setup({});
    for (const args of [{ from: '123', to: 'x' }, { from: 'a', to: 'b c' }, { from: 'a', to: '#a' }]) {
      const res = await call(args);
      expect(res.isError).toBe(true);
    }
  });

  it('reports a failed write and keeps going', async () => {
    const call = await setup({ 'a/A.md': '#old', 'B.md': '#old' });
    await chmod(join(fx!.vault, 'a'), 0o500);
    try {
      const out = unwrap(await call({ from: 'old', to: 'new', dryRun: false }));
      expect(out.files).toEqual([{ path: 'B.md', inline: 1, frontmatter: 0 }]);
      expect(out.failed).toEqual([{ path: 'a/A.md', error: expect.any(String) }]);
    } finally {
      await chmod(join(fx!.vault, 'a'), 0o700);
    }
  });
});
