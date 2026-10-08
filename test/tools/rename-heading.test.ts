import { afterEach, describe, expect, it } from 'vitest';
import { registerRenameHeadingTool, rewriteHeadingLinks } from '../../src/tools/rename-heading.js';
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
  registerRenameHeadingTool(server, fx.ctx);
  return (args) => registered[0]!.cb(args);
}

const NOTE = '---\ntitle: Note\n---\n# Parent\n## Old ##\nbody, see [[#Old]] and [[#Parent]]\n';

describe('rename_heading', () => {
  it('renames the heading and every link form across the vault', async () => {
    const call = await setup({
      'Note.md': NOTE,
      'A.md': '[[Note#Old]] [[Note#Old|alias]] ![[Note#Old]] [[Note#Parent#Old]] [[Note#old]]',
      'B.md': '[[Note#Other]] [[Other#Old]] `[[Note#Old]]` [[Note#^Old]] [[#Old]]',
      'Other.md': '## Old',
    });
    const out = unwrap(await call({ name: 'Note', from: 'Old', to: 'New Name' }));
    expect(out).toEqual({
      path: 'Note.md',
      from: 'Old',
      to: 'New Name',
      filesChanged: ['A.md', 'Note.md'],
      linksRewritten: 6,
    });
    expect(await fx!.read('Note.md')).toBe(
      '---\ntitle: Note\n---\n# Parent\n## New Name ##\nbody, see [[#New Name]] and [[#Parent]]\n',
    );
    expect(await fx!.read('A.md')).toBe(
      '[[Note#New Name]] [[Note#New Name|alias]] ![[Note#New Name]] [[Note#Parent#New Name]] [[Note#New Name]]',
    );
    expect(await fx!.read('B.md')).toBe('[[Note#Other]] [[Other#Old]] `[[Note#Old]]` [[Note#^Old]] [[#Old]]');
    expect(fx!.ctx.reindexCalls).toBe(1);
  });

  it('dryRun reports the plan and writes nothing', async () => {
    const call = await setup({ 'Note.md': NOTE, 'A.md': '[[Note#Old]]' });
    const out = unwrap(await call({ name: 'Note.md', from: 'Old', to: 'New', dryRun: true }));
    expect(out).toMatchObject({ dryRun: true, filesChanged: ['A.md', 'Note.md'], linksRewritten: 2 });
    expect(await fx!.read('Note.md')).toBe(NOTE);
    expect(await fx!.read('A.md')).toBe('[[Note#Old]]');
    expect(fx!.ctx.reindexCalls).toBe(0);
  });

  it('a parent rename rewrites the parent segment of nested links', async () => {
    const call = await setup({ 'Note.md': NOTE, 'A.md': '[[Note#Parent#Old]]' });
    unwrap(await call({ name: 'Note', from: 'Parent', to: 'Top' }));
    expect(await fx!.read('A.md')).toBe('[[Note#Top#Old]]');
  });

  it('errors on a missing heading, a duplicate heading, a collision and a bad name', async () => {
    const call = await setup({ 'Note.md': NOTE, 'Dup.md': '# Same\n# Same\n' });
    const cases: Array<[Record<string, unknown>, RegExp]> = [
      [{ name: 'Note', from: 'Nope', to: 'X' }, /not found in Note\.md/],
      [{ name: 'Note', from: 'Old', to: 'parent' }, /already exists/],
      [{ name: 'Note', from: 'Old', to: 'A#B' }, /cannot contain/],
      [{ name: 'Dup', from: 'Same', to: 'X' }, /appears 2 times .*lines 1, 2/],
      [{ name: 'Nowhere', from: 'Old', to: 'X' }, /No note found/],
    ];
    for (const [args, msg] of cases) {
      const res = await call(args);
      expect(res.isError).toBe(true);
      expect(res.content[0].text).toMatch(msg);
    }
    expect(await fx!.read('Note.md')).toBe(NOTE);
  });

  it('errors on an ambiguous note name', async () => {
    const call = await setup({ 'a/Topic one.md': '# H', 'b/Topic two.md': '# H' });
    const res = await call({ name: 'topic', from: 'H', to: 'X' });
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toMatch(/Multiple notes match/);
  });

  it('allows a case-only rename of the same heading', async () => {
    const call = await setup({ 'Note.md': '# old\n[[#old]]' });
    unwrap(await call({ name: 'Note', from: 'old', to: 'Old' }));
    expect(await fx!.read('Note.md')).toBe('# Old\n[[#Old]]');
  });
});

describe('rewriteHeadingLinks', () => {
  it('only touches links the predicate accepts and keeps frontmatter', () => {
    const raw = '---\nx: "[[N#A]]"\n---\n[[N#A]] [[M#A]] [[N]]';
    expect(rewriteHeadingLinks(raw, 'a', 'B', (t) => t === 'N')).toEqual({
      text: '---\nx: "[[N#A]]"\n---\n[[N#B]] [[M#A]] [[N]]',
      links: 1,
    });
  });
});
