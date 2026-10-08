import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdtemp, mkdir, writeFile, readFile, rm, symlink, readdir } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { registerReadCanvasTool } from '../../src/tools/read-canvas.js';
import { registerEditCanvasTool } from '../../src/tools/edit-canvas.js';
import type { ServerContext } from '../../src/context.js';
import { makeMockServer, unwrap } from '../helpers/mock-server.js';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function unwrapError(result: any): string {
  expect(result.isError).toBe(true);
  return result.content[0].text as string;
}

const BOARD = {
  nodes: [
    { id: 'aaaaaaaaaaaaaaaa', type: 'text', text: 'Hello', x: 0, y: 0, width: 250, height: 60, styleAttributes: { textAlign: 'center' } },
    { id: 'bbbbbbbbbbbbbbbb', type: 'file', file: 'Notes/Idea.md', x: 300, y: -40, width: 400, height: 400 },
    { id: 'cccccccccccccccc', type: 'group', label: 'Box', x: -100, y: 500, width: 200, height: 100 },
  ],
  edges: [
    { id: 'eeeeeeeeeeeeeeee', fromNode: 'aaaaaaaaaaaaaaaa', fromSide: 'right', toNode: 'bbbbbbbbbbbbbbbb', toSide: 'left', custom: 1 },
  ],
};

describe('canvas tools', () => {
  let vault: string;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let read: (args: any) => Promise<any>;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let edit: (args: any) => Promise<any>;

  beforeEach(async () => {
    vault = await mkdtemp(join(tmpdir(), 'kg-canvas-'));
    await mkdir(join(vault, 'Notes'));
    await writeFile(join(vault, 'Notes', 'Idea.md'), '# Idea\n');
    await writeFile(join(vault, 'Board.canvas'), JSON.stringify(BOARD, null, 2));
    const ctx = { config: { vaultPath: vault } } as unknown as ServerContext;
    const { server, registered } = makeMockServer();
    registerReadCanvasTool(server, ctx);
    registerEditCanvasTool(server, ctx);
    read = registered.find((t) => t.name === 'read_canvas')!.cb;
    edit = registered.find((t) => t.name === 'edit_canvas')!.cb;
  });

  afterEach(async () => {
    await rm(vault, { recursive: true, force: true });
  });

  async function onDisk(path = 'Board.canvas'): Promise<{ raw: string; doc: typeof BOARD }> {
    const raw = await readFile(join(vault, path), 'utf-8');
    return { raw, doc: JSON.parse(raw) };
  }

  describe('read_canvas', () => {
    it('returns nodes and edges, unknown fields included', async () => {
      const out = unwrap(await read({ path: 'Board.canvas' }));
      expect(out.path).toBe('Board.canvas');
      expect(out.nodes).toEqual(BOARD.nodes);
      expect(out.edges).toEqual(BOARD.edges);
    });

    it('appends the .canvas extension when missing', async () => {
      const out = unwrap(await read({ path: 'Board' }));
      expect(out.nodes).toHaveLength(3);
    });

    it('reads an empty file as an empty canvas', async () => {
      await writeFile(join(vault, 'Empty.canvas'), '');
      expect(unwrap(await read({ path: 'Empty.canvas' }))).toEqual({ path: 'Empty.canvas', nodes: [], edges: [] });
    });

    it('treats missing nodes or edges keys as empty', async () => {
      await writeFile(join(vault, 'Half.canvas'), '{"nodes":[]}');
      expect(unwrap(await read({ path: 'Half' })).edges).toEqual([]);
    });

    it('errors on a missing canvas', async () => {
      expect(unwrapError(await read({ path: 'Nope.canvas' }))).toMatch(/Canvas not found: Nope\.canvas/);
    });

    it('errors on invalid JSON', async () => {
      await writeFile(join(vault, 'Bad.canvas'), '{nodes: [');
      expect(unwrapError(await read({ path: 'Bad.canvas' }))).toMatch(/Bad\.canvas is not valid JSON/);
    });

    it('errors on a JSON value that is not an object', async () => {
      await writeFile(join(vault, 'Arr.canvas'), '[]');
      expect(unwrapError(await read({ path: 'Arr.canvas' }))).toMatch(/not a JSON object/);
    });

    it('errors on a non-array nodes field', async () => {
      await writeFile(join(vault, 'Odd.canvas'), '{"nodes":{},"edges":[]}');
      expect(unwrapError(await read({ path: 'Odd.canvas' }))).toMatch(/non-array "nodes"/);
    });

    it('refuses paths outside the vault', async () => {
      expect(unwrapError(await read({ path: '../outside.canvas' }))).toMatch(/outside the vault/);
      expect(unwrapError(await read({ path: '/etc/x.canvas' }))).toMatch(/vault-relative/);
    });

    it('refuses a symlink that leads out of the vault', async () => {
      const outside = await mkdtemp(join(tmpdir(), 'kg-canvas-out-'));
      try {
        await writeFile(join(outside, 'Secret.canvas'), '{}');
        await symlink(outside, join(vault, 'link'));
        expect(unwrapError(await read({ path: 'link/Secret.canvas' }))).toMatch(/through a symlink/);
      } finally {
        await rm(outside, { recursive: true, force: true });
      }
    });
  });

  describe('edit_canvas add_node', () => {
    it('adds a text node right of the right-most node with default size', async () => {
      const out = unwrap(await edit({ path: 'Board.canvas', operation: 'add_node', type: 'text', text: 'New' }));
      expect(out.node).toMatchObject({ type: 'text', text: 'New', x: 740, y: -40, width: 400, height: 200 });
      expect(out.node.id).toMatch(/^[0-9a-f]{16}$/);
      expect(out.created).toBe(false);
      const { doc } = await onDisk();
      expect(doc.nodes).toHaveLength(4);
      expect(doc.nodes[3]).toEqual(out.node);
      expect(doc.nodes[0]).toEqual(BOARD.nodes[0]);
      expect(doc.edges).toEqual(BOARD.edges);
    });

    it('writes the Obsidian layout: tab-indented, one element per line', async () => {
      await edit({ path: 'Board.canvas', operation: 'add_node', type: 'text', text: 'New' });
      const { raw, doc } = await onDisk();
      const lines = raw.split('\n');
      expect(lines[0]).toBe('{');
      expect(lines[1]).toBe('\t"nodes":[');
      expect(lines[2]).toBe(`\t\t${JSON.stringify(doc.nodes[0])},`);
      expect(raw).toContain(`\t"edges":[\n\t\t${JSON.stringify(doc.edges[0])}\n\t]\n}`);
    });

    it('keeps extra top-level keys', async () => {
      await writeFile(join(vault, 'Meta.canvas'), '{"nodes":[],"edges":[],"metadata":{"v":1}}');
      await edit({ path: 'Meta.canvas', operation: 'add_node', type: 'group' });
      const { raw, doc } = await onDisk('Meta.canvas');
      expect((doc as unknown as { metadata: unknown }).metadata).toEqual({ v: 1 });
      expect(raw).toContain('\t"metadata":{"v":1}');
    });

    it('creates a missing canvas, its folders, and places the first node at the origin', async () => {
      const out = unwrap(
        await edit({ path: 'Boards/New', operation: 'add_node', type: 'link', url: 'https://example.com' }),
      );
      expect(out.path).toBe('Boards/New.canvas');
      expect(out.created).toBe(true);
      expect(out.node).toMatchObject({ type: 'link', url: 'https://example.com', x: 0, y: 0 });
      const { raw, doc } = await onDisk('Boards/New.canvas');
      expect(doc.nodes).toEqual([out.node]);
      expect(raw).toContain('\t"edges":[]');
      expect((await readdir(join(vault, 'Boards'))).filter((f) => f.endsWith('.tmp'))).toEqual([]);
    });

    it('honours explicit position, size, color and group label', async () => {
      const out = unwrap(
        await edit({
          path: 'Board.canvas', operation: 'add_node', type: 'group',
          label: 'Cluster', color: '4', x: 10, y: 20, width: 800, height: 600,
        }),
      );
      expect(out.node).toMatchObject({ type: 'group', label: 'Cluster', color: '4', x: 10, y: 20, width: 800, height: 600 });
    });

    it('adds a file node with subpath without a warning when the file exists', async () => {
      const out = unwrap(
        await edit({ path: 'Board.canvas', operation: 'add_node', type: 'file', file: 'Notes/Idea.md', subpath: '#Idea' }),
      );
      expect(out.node).toMatchObject({ file: 'Notes/Idea.md', subpath: '#Idea' });
      expect(out.warnings).toBeUndefined();
    });

    it('warns but still adds a file node whose target is missing', async () => {
      const out = unwrap(await edit({ path: 'Board.canvas', operation: 'add_node', type: 'file', file: 'Missing.md' }));
      expect(out.warnings).toEqual(['File not found in the vault: Missing.md']);
      expect((await onDisk()).doc.nodes).toHaveLength(4);
    });

    it('refuses a file node pointing outside the vault', async () => {
      const err = unwrapError(await edit({ path: 'Board.canvas', operation: 'add_node', type: 'file', file: '../x.md' }));
      expect(err).toMatch(/outside the vault/);
      expect((await onDisk()).doc.nodes).toHaveLength(3);
    });

    it('ignores null optional fields', async () => {
      const out = unwrap(await edit({ path: 'Board.canvas', operation: 'add_node', type: 'text', text: 'x', color: null }));
      expect('color' in out.node).toBe(false);
    });

    it('requires a type and the content field of that type', async () => {
      expect(unwrapError(await edit({ path: 'Board.canvas', operation: 'add_node', text: 'x' }))).toMatch(/needs `type`/);
      expect(unwrapError(await edit({ path: 'Board.canvas', operation: 'add_node', type: 'text' }))).toMatch(/text node needs `text`/);
      expect(unwrapError(await edit({ path: 'Board.canvas', operation: 'add_node', type: 'file' }))).toMatch(/file node needs `file`/);
      expect(unwrapError(await edit({ path: 'Board.canvas', operation: 'add_node', type: 'link' }))).toMatch(/link node needs `url`/);
    });

    it('refuses content fields of another node type', async () => {
      const err = unwrapError(await edit({ path: 'Board.canvas', operation: 'add_node', type: 'text', text: 'x', url: 'https://a' }));
      expect(err).toMatch(/`url` does not apply to a text node/);
    });

    it('refuses arguments that belong to another operation', async () => {
      const err = unwrapError(await edit({ path: 'Board.canvas', operation: 'add_node', type: 'text', text: 'x', edgeId: 'e' }));
      expect(err).toMatch(/`edgeId` does not apply to add_node/);
    });

    it('generates ids unique against existing nodes and edges', async () => {
      const ids = new Set<string>();
      for (let i = 0; i < 5; i++) {
        ids.add(unwrap(await edit({ path: 'Board.canvas', operation: 'add_node', type: 'text', text: `${i}` })).node.id);
      }
      expect(ids.size).toBe(5);
    });

    it('places next to nodes with missing geometry without producing NaN', async () => {
      await writeFile(join(vault, 'Loose.canvas'), '{"nodes":[{"id":"x","type":"text","text":"a"}],"edges":[]}');
      const out = unwrap(await edit({ path: 'Loose', operation: 'add_node', type: 'text', text: 'b' }));
      expect(out.node).toMatchObject({ x: 40, y: 0 });
    });
  });

  describe('edit_canvas update_node', () => {
    it('changes only the named fields and keeps unknown ones', async () => {
      const out = unwrap(
        await edit({ path: 'Board.canvas', operation: 'update_node', nodeId: 'aaaaaaaaaaaaaaaa', text: 'Bye', x: 5, color: '#ff0000' }),
      );
      expect(out.node).toEqual({ ...BOARD.nodes[0], text: 'Bye', x: 5, color: '#ff0000' });
      expect((await onDisk()).doc.nodes[0]).toEqual(out.node);
    });

    it('removes optional fields given as null', async () => {
      await edit({ path: 'Board.canvas', operation: 'update_node', nodeId: 'cccccccccccccccc', label: null });
      expect('label' in (await onDisk()).doc.nodes[2]!).toBe(false);
    });

    it('refuses to remove a required content field', async () => {
      const err = unwrapError(await edit({ path: 'Board.canvas', operation: 'update_node', nodeId: 'aaaaaaaaaaaaaaaa', text: null }));
      expect(err).toMatch(/`text` is required on a text node/);
    });

    it('warns when a file node is pointed at a missing file', async () => {
      const out = unwrap(await edit({ path: 'Board.canvas', operation: 'update_node', nodeId: 'bbbbbbbbbbbbbbbb', file: 'Gone.md' }));
      expect(out.node.file).toBe('Gone.md');
      expect(out.warnings).toEqual(['File not found in the vault: Gone.md']);
    });

    it('refuses fields of another node type', async () => {
      const err = unwrapError(await edit({ path: 'Board.canvas', operation: 'update_node', nodeId: 'bbbbbbbbbbbbbbbb', text: 'x' }));
      expect(err).toMatch(/`text` does not apply to a file node/);
    });

    it('allows any content field on node types it does not know', async () => {
      await writeFile(join(vault, 'Ext.canvas'), '{"nodes":[{"id":"z","type":"custom","x":0,"y":0,"width":1,"height":1}]}');
      const out = unwrap(await edit({ path: 'Ext', operation: 'update_node', nodeId: 'z', text: 'ok' }));
      expect(out.node.text).toBe('ok');
    });

    it('errors without nodeId, on an unknown node, and with nothing to change', async () => {
      expect(unwrapError(await edit({ path: 'Board.canvas', operation: 'update_node', x: 1 }))).toMatch(/needs `nodeId`/);
      expect(unwrapError(await edit({ path: 'Board.canvas', operation: 'update_node', nodeId: 'nope', x: 1 }))).toMatch(/No node with id "nope"/);
      expect(unwrapError(await edit({ path: 'Board.canvas', operation: 'update_node', nodeId: 'aaaaaaaaaaaaaaaa' }))).toMatch(/at least one field/);
    });

    it('errors when the canvas does not exist', async () => {
      const err = unwrapError(await edit({ path: 'Nope', operation: 'update_node', nodeId: 'a', x: 1 }));
      expect(err).toMatch(/Canvas not found: Nope\.canvas/);
      expect(existsSync(join(vault, 'Nope.canvas'))).toBe(false);
    });
  });

  describe('edit_canvas delete_node', () => {
    it('removes the node and every edge touching it', async () => {
      const out = unwrap(await edit({ path: 'Board.canvas', operation: 'delete_node', nodeId: 'bbbbbbbbbbbbbbbb' }));
      expect(out.node.id).toBe('bbbbbbbbbbbbbbbb');
      expect(out.removedEdges.map((e: { id: string }) => e.id)).toEqual(['eeeeeeeeeeeeeeee']);
      const { doc } = await onDisk();
      expect(doc.nodes.map((n) => n.id)).toEqual(['aaaaaaaaaaaaaaaa', 'cccccccccccccccc']);
      expect(doc.edges).toEqual([]);
    });

    it('errors on an unknown node', async () => {
      expect(unwrapError(await edit({ path: 'Board.canvas', operation: 'delete_node', nodeId: 'x' }))).toMatch(/No node with id/);
    });
  });

  describe('edit_canvas connect / disconnect', () => {
    it('connects two nodes with sides, ends, label and color', async () => {
      const out = unwrap(
        await edit({
          path: 'Board.canvas', operation: 'connect', fromNode: 'cccccccccccccccc', toNode: 'aaaaaaaaaaaaaaaa',
          fromSide: 'top', toSide: 'bottom', fromEnd: 'arrow', toEnd: 'none', label: 'leads to', color: '2',
        }),
      );
      expect(out.edge).toMatchObject({
        fromNode: 'cccccccccccccccc', toNode: 'aaaaaaaaaaaaaaaa', fromSide: 'top', toSide: 'bottom',
        fromEnd: 'arrow', toEnd: 'none', label: 'leads to', color: '2',
      });
      expect(out.edge.id).toMatch(/^[0-9a-f]{16}$/);
      const { doc } = await onDisk();
      expect(doc.edges).toEqual([BOARD.edges[0], out.edge]);
    });

    it('writes a minimal edge when only the ends are given', async () => {
      const out = unwrap(await edit({ path: 'Board.canvas', operation: 'connect', fromNode: 'aaaaaaaaaaaaaaaa', toNode: 'cccccccccccccccc', label: null }));
      expect(Object.keys(out.edge).sort()).toEqual(['fromNode', 'id', 'toNode']);
    });

    it('requires both nodes to exist', async () => {
      expect(unwrapError(await edit({ path: 'Board.canvas', operation: 'connect', toNode: 'aaaaaaaaaaaaaaaa' }))).toMatch(/needs `fromNode`/);
      expect(unwrapError(await edit({ path: 'Board.canvas', operation: 'connect', fromNode: 'aaaaaaaaaaaaaaaa', toNode: 'zz' }))).toMatch(/No node with id "zz"/);
    });

    it('refuses node fields on connect', async () => {
      const err = unwrapError(await edit({ path: 'Board.canvas', operation: 'connect', fromNode: 'a', toNode: 'b', x: 1 }));
      expect(err).toMatch(/`x` does not apply to connect/);
    });

    it('disconnects an edge by id', async () => {
      const out = unwrap(await edit({ path: 'Board.canvas', operation: 'disconnect', edgeId: 'eeeeeeeeeeeeeeee' }));
      expect(out.edge).toEqual(BOARD.edges[0]);
      const { doc } = await onDisk();
      expect(doc.edges).toEqual([]);
      expect(doc.nodes).toHaveLength(3);
    });

    it('errors on a missing or unknown edge id', async () => {
      expect(unwrapError(await edit({ path: 'Board.canvas', operation: 'disconnect' }))).toMatch(/needs `edgeId`/);
      expect(unwrapError(await edit({ path: 'Board.canvas', operation: 'disconnect', edgeId: 'x' }))).toMatch(/No edge with id "x"/);
    });
  });

  it('leaves an invalid canvas untouched', async () => {
    await writeFile(join(vault, 'Bad.canvas'), 'not json');
    expect(unwrapError(await edit({ path: 'Bad', operation: 'add_node', type: 'text', text: 'x' }))).toMatch(/not valid JSON/);
    expect(await readFile(join(vault, 'Bad.canvas'), 'utf-8')).toBe('not json');
  });
});
