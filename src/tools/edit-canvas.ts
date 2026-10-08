import { z } from 'zod';
import { existsSync } from 'node:fs';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import type { ServerContext } from '../context.js';
import {
  CONTENT_FIELDS,
  DEFAULT_NODE_HEIGHT,
  DEFAULT_NODE_WIDTH,
  canvasPath,
  generateCanvasId,
  placeRightOf,
  readCanvas,
  writeCanvas,
  type CanvasDocument,
  type CanvasEdge,
  type CanvasNode,
  type CanvasNodeType,
} from '../vault/canvas.js';
import { resolveInVault } from '../vault/paths.js';

type Operation = 'add_node' | 'update_node' | 'delete_node' | 'connect' | 'disconnect';

const NODE_FIELDS = ['text', 'file', 'subpath', 'url', 'label', 'color', 'x', 'y', 'width', 'height'] as const;
const CONTENT_KEYS = ['text', 'file', 'subpath', 'url', 'label'] as const;
const REQUIRED_CONTENT: Partial<Record<CanvasNodeType, string>> = { text: 'text', file: 'file', link: 'url' };

/** Arguments each operation accepts besides `operation` and `path`. */
const ALLOWED_ARGS: Record<Operation, readonly string[]> = {
  add_node: ['type', ...NODE_FIELDS],
  update_node: ['nodeId', ...NODE_FIELDS],
  delete_node: ['nodeId'],
  connect: ['fromNode', 'toNode', 'fromSide', 'toSide', 'fromEnd', 'toEnd', 'label', 'color'],
  disconnect: ['edgeId'],
};

const side = z.enum(['top', 'right', 'bottom', 'left']);
const end = z.enum(['none', 'arrow']);

/**
 * `edit_canvas` — one structured change to a `.canvas` file per call: add,
 * update or delete a node, or add or remove an edge. Fields the call does
 * not name stay as they are, including ones this server does not model.
 * Canvases are not part of the note index, so no reindex follows.
 */
export function registerEditCanvasTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'edit_canvas',
    'Change an Obsidian canvas: `add_node` (creates the canvas if missing; placed right of the right-most node by default), `update_node`, `delete_node` (also removes its edges), `connect` or `disconnect`. Returns the changed node or edge.',
    {
      path: z.string().min(1).describe('Vault-relative path of the `.canvas` file. The extension is optional.'),
      operation: z.enum(['add_node', 'update_node', 'delete_node', 'connect', 'disconnect']),
      nodeId: z.string().optional().describe('Node to change (`update_node`, `delete_node`).'),
      type: z.enum(['text', 'file', 'link', 'group']).optional().describe('Node type (`add_node`).'),
      text: z.string().optional().describe('Markdown content of a `text` node.'),
      file: z.string().optional().describe('Vault-relative file a `file` node shows.'),
      subpath: z.string().nullable().optional().describe('Heading or block of a `file` node, e.g. `#Heading`. `null` removes it.'),
      url: z.string().optional().describe('URL of a `link` node.'),
      label: z.string().nullable().optional().describe('Label of a `group` node or an edge. `null` removes it.'),
      color: z.string().nullable().optional().describe('Preset `"1"`-`"6"` or hex like `"#ff0000"`. `null` removes it.'),
      x: z.number().optional(),
      y: z.number().optional(),
      width: z.number().positive().optional().describe('Default 400.'),
      height: z.number().positive().optional().describe('Default 200.'),
      fromNode: z.string().optional().describe('Edge start node id (`connect`).'),
      toNode: z.string().optional().describe('Edge end node id (`connect`).'),
      fromSide: side.optional(),
      toSide: side.optional(),
      fromEnd: end.optional().describe('Default `none`.'),
      toEnd: end.optional().describe('Default `arrow`.'),
      edgeId: z.string().optional().describe('Edge to remove (`disconnect`).'),
    },
    async (args) => {
      const { operation } = args;
      const path = canvasPath(args.path);
      const given = Object.entries(args)
        .filter(([key, value]) => value !== undefined && key !== 'operation' && key !== 'path')
        .map(([key]) => key);
      const stray = given.filter((key) => !ALLOWED_ARGS[operation].includes(key));
      if (stray.length > 0) {
        throw new Error(`${stray.map((k) => `\`${k}\``).join(', ')} does not apply to ${operation}`);
      }

      const existing = await readCanvas(ctx.config.vaultPath, path);
      if (!existing && operation !== 'add_node') throw new Error(`Canvas not found: ${path}`);
      const doc: CanvasDocument = existing ?? { nodes: [], edges: [] };
      const warnings: string[] = [];
      let result: Record<string, unknown>;

      switch (operation) {
        case 'add_node': {
          const type = args.type;
          if (!type) throw new Error('add_node needs `type`');
          checkContentFields(type, args);
          const required = REQUIRED_CONTENT[type];
          if (required && args[required as keyof typeof args] === undefined) {
            throw new Error(`A ${type} node needs \`${required}\``);
          }
          const placed = placeRightOf(doc.nodes);
          const node: CanvasNode = {
            id: generateCanvasId(doc),
            type,
            x: args.x ?? placed.x,
            y: args.y ?? placed.y,
            width: args.width ?? DEFAULT_NODE_WIDTH,
            height: args.height ?? DEFAULT_NODE_HEIGHT,
          };
          for (const key of [...CONTENT_KEYS, 'color'] as const) {
            const value = args[key];
            if (value !== undefined && value !== null) node[key] = value;
          }
          if (node.file !== undefined) checkFileTarget(ctx, node.file, warnings);
          doc.nodes.push(node);
          result = { node, created: existing === null };
          break;
        }

        case 'update_node': {
          const node = findNode(doc, args.nodeId, 'update_node');
          const changes = given.filter((key) => key !== 'nodeId');
          if (changes.length === 0) throw new Error('update_node needs at least one field to change');
          checkContentFields(node.type, args);
          const fields: Record<string, unknown> = node;
          for (const key of NODE_FIELDS) {
            const value = args[key];
            if (value === undefined) continue;
            if (value === null) {
              delete fields[key];
            } else {
              fields[key] = value;
            }
          }
          if (args.file !== undefined) checkFileTarget(ctx, args.file, warnings);
          result = { node };
          break;
        }

        case 'delete_node': {
          const node = findNode(doc, args.nodeId, 'delete_node');
          doc.nodes = doc.nodes.filter((n) => n !== node);
          const removedEdges = doc.edges.filter((e) => e.fromNode === node.id || e.toNode === node.id);
          doc.edges = doc.edges.filter((e) => !removedEdges.includes(e));
          result = { node, removedEdges };
          break;
        }

        case 'connect': {
          const from = findNode(doc, args.fromNode, 'connect', 'fromNode');
          const to = findNode(doc, args.toNode, 'connect', 'toNode');
          const edge: CanvasEdge = { id: generateCanvasId(doc), fromNode: from.id, toNode: to.id };
          const fields: Record<string, unknown> = edge;
          for (const key of ['fromSide', 'toSide', 'fromEnd', 'toEnd', 'color', 'label'] as const) {
            const value = args[key];
            if (value !== undefined && value !== null) fields[key] = value;
          }
          doc.edges.push(edge);
          result = { edge };
          break;
        }

        case 'disconnect': {
          if (!args.edgeId) throw new Error('disconnect needs `edgeId`');
          const edge = doc.edges.find((e) => e.id === args.edgeId);
          if (!edge) throw new Error(`No edge with id "${args.edgeId}" in ${path}`);
          doc.edges = doc.edges.filter((e) => e !== edge);
          result = { edge };
          break;
        }
      }

      await writeCanvas(ctx.config.vaultPath, path, doc);
      return { path, operation, ...result, ...(warnings.length > 0 ? { warnings } : {}) };
    },
  );
}

function findNode(
  doc: CanvasDocument,
  id: string | undefined,
  operation: string,
  argName = 'nodeId',
): CanvasNode {
  if (!id) throw new Error(`${operation} needs \`${argName}\``);
  const node = doc.nodes.find((n) => n.id === id);
  if (!node) throw new Error(`No node with id "${id}"`);
  return node;
}

/** Refuse content fields that belong to another node type, and removing required ones. */
function checkContentFields(type: string, args: Partial<Record<string, unknown>>): void {
  const own = CONTENT_FIELDS[type as CanvasNodeType];
  if (!own) return;
  for (const key of CONTENT_KEYS) {
    if (args[key] === undefined) continue;
    if (!own.includes(key)) throw new Error(`\`${key}\` does not apply to a ${type} node`);
    if (args[key] === null && REQUIRED_CONTENT[type as CanvasNodeType] === key) {
      throw new Error(`\`${key}\` is required on a ${type} node and cannot be removed`);
    }
  }
}

/** The target must stay inside the vault; a missing target only warns. */
function checkFileTarget(ctx: ServerContext, file: string, warnings: string[]): void {
  const abs = resolveInVault(ctx.config.vaultPath, file);
  if (!existsSync(abs)) warnings.push(`File not found in the vault: ${file}`);
}
