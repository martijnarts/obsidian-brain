import { afterEach, describe, expect, it } from 'vitest';
import { rm, symlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { upsertNode } from '../../src/store/nodes.js';
import { registerFindBrokenLinksTool } from '../../src/tools/find-broken-links.js';
import { makeMockServer, unwrap } from '../helpers/mock-server.js';
import { makeVault, type VaultFixture } from '../helpers/vault-fixture.js';

let fx: VaultFixture | undefined;
afterEach(async () => {
  await fx?.cleanup();
  fx = undefined;
});

// eslint-disable-next-line @typescript-eslint/no-explicit-any
async function run(files: Record<string, string>, args: Record<string, unknown> = {}): Promise<any> {
  fx = await makeVault(files);
  const { server, registered } = makeMockServer();
  registerFindBrokenLinksTool(server, fx.ctx);
  return unwrap(await registered[0]!.cb(args));
}

describe('find_broken_links', () => {
  it('returns nothing for a vault whose links all resolve', async () => {
    const out = await run({
      'A.md': 'See [[B]] and [[B#Intro]] and [[B#^blk]] and [[B|alias]].',
      'B.md': '# Intro\n\nA paragraph ^blk\n',
    });
    expect(out).toEqual({ scannedFiles: 2, total: 0, brokenLinks: [] });
  });

  it('reports a missing note with the 1-based line in the file, frontmatter included', async () => {
    const out = await run({
      'A.md': '---\ntitle: A\n---\nline one\nsee [[Missing|m]] here\n',
    });
    expect(out.brokenLinks).toEqual([
      { source: 'A.md', line: 5, link: '[[Missing|m]]', reason: 'note_not_found' },
    ]);
  });

  it('reports missing headings and block ids in an existing target', async () => {
    const out = await run({
      'A.md': '[[B#Nope]]\n[[B#^gone]]\n[[B^gone2]]\n[[B#Real]]\n[[B#real]]',
      'B.md': '## Real ##\n```\n# Fake\nx ^fakeblk\n```\n',
    });
    expect(out.brokenLinks.map((b: { link: string; reason: string }) => [b.link, b.reason])).toEqual([
      ['[[B#Nope]]', 'heading_not_found'],
      ['[[B#^gone]]', 'block_not_found'],
      ['[[B^gone2]]', 'block_not_found'],
    ]);
  });

  it('checks same-note links and nested heading paths', async () => {
    const out = await run({
      'A.md': '# Top\n## Sub\n[[#Top]] [[#Sub]] [[#Missing]] [[A#Top#Sub]] [[A#Top#Nope]] [[#^x]]',
    });
    expect(out.brokenLinks.map((b: { link: string }) => b.link)).toEqual([
      '[[#Missing]]',
      '[[A#Top#Nope]]',
      '[[#^x]]',
    ]);
  });

  it('skips links inside code, embeds, and attachments that exist on disk', async () => {
    const out = await run({
      'A.md': '`[[Gone1]]`\n```\n[[Gone2]]\n```\n![[Gone3]]\n[[file.pdf]] [[sub/pic.png]] [[missing.pdf]] [[B#]]',
      'file.pdf': 'pdf',
      'assets/pic.png': 'png',
      '.hidden/missing.pdf': 'x',
      'B.md': 'b',
    });
    expect(out.brokenLinks.map((b: { link: string }) => b.link)).toEqual(['[[missing.pdf]]']);
  });

  it('filters by folder and excludeFolders, and applies limit', async () => {
    const files = {
      'a/One.md': '[[X1]] [[X2]] [[X3]]',
      'a/skip/Two.md': '[[X4]]',
      'b/Three.md': '[[X5]]',
    };
    const out = await run(files, { folder: '/a/', excludeFolders: ['a/skip'], limit: 2 });
    expect(out.scannedFiles).toBe(1);
    expect(out.total).toBe(3);
    expect(out.truncated).toBe(true);
    expect(out.brokenLinks).toHaveLength(2);
  });

  it('skips a file that vanished or links outside the vault', async () => {
    fx = await makeVault({ 'A.md': '[[Gone]]', 'B.md': '[[Gone]]' });
    await rm(join(fx.vault, 'B.md'));
    const outside = join(fx.vault, '..', `outside-${Date.now()}.md`);
    await writeFile(outside, '[[Gone]]');
    await symlink(outside, join(fx.vault, 'C.md'));
    upsertNode(fx.db, { id: 'C.md', title: 'C', content: '', frontmatter: {} });
    const { server, registered } = makeMockServer();
    registerFindBrokenLinksTool(server, fx.ctx);
    const out = unwrap(await registered[0]!.cb({}));
    expect(out.scannedFiles).toBe(1);
    expect(out.brokenLinks.map((b: { source: string }) => b.source)).toEqual(['A.md']);
    await rm(outside);
  });
});
