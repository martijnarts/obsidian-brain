import { afterEach, describe, expect, it } from 'vitest';
import { registerFindOrphanedNotesTool } from '../../src/tools/find-orphaned-notes.js';
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
  registerFindOrphanedNotesTool(server, fx.ctx);
  return unwrap(await registered[0]!.cb(args));
}

describe('find_orphaned_notes', () => {
  it('lists notes without incoming links; self-links and stubs do not count', async () => {
    const out = await run({
      'Hub.md': '---\ntitle: The Hub\n---\n[[Leaf]] [[Hub]] [[Missing]] [[Leaf]]',
      'Leaf.md': 'no links',
      'Lonely.md': '[[Lonely]]',
    });
    expect(out).toEqual({
      total: 2,
      orphans: [
        { path: 'Hub.md', title: 'The Hub', outgoingLinks: 3 },
        { path: 'Lonely.md', title: 'Lonely', outgoingLinks: 0 },
      ],
    });
  });

  it('hides excluded folders from the result but counts their links', async () => {
    const out = await run(
      {
        'templates/T.md': '[[Used]]',
        'Used.md': '',
        'notes/Free.md': '',
      },
      { excludeFolders: ['/templates/'] },
    );
    expect(out.orphans.map((o: { path: string }) => o.path)).toEqual(['notes/Free.md']);
  });

  it('filters by folder and caps at limit', async () => {
    const out = await run({ 'a/1.md': '', 'a/2.md': '', 'a/3.md': '', 'b/4.md': '' }, { folder: 'a', limit: 2 });
    expect(out.total).toBe(3);
    expect(out.truncated).toBe(true);
    expect(out.orphans.map((o: { path: string }) => o.path)).toEqual(['a/1.md', 'a/2.md']);
  });

  it('returns an empty list for an empty vault', async () => {
    expect(await run({})).toEqual({ total: 0, orphans: [] });
  });
});
