/**
 * Read and write `.canvas` files (JSON Canvas 1.0, https://jsoncanvas.org).
 *
 * Nodes and edges are kept as plain objects: a write changes only the
 * fields a tool touches, so styling, plugin data and future spec fields
 * survive a round trip.
 */

import { randomBytes } from 'crypto';
import { promises as fs } from 'fs';
import { dirname } from 'path';
import { resolveInVault } from './paths.js';

export type CanvasNodeType = 'text' | 'file' | 'link' | 'group';
export type CanvasSide = 'top' | 'right' | 'bottom' | 'left';
export type CanvasEnd = 'none' | 'arrow';

export interface CanvasNode {
  id: string;
  type: CanvasNodeType | string;
  x: number;
  y: number;
  width: number;
  height: number;
  color?: string;
  text?: string;
  file?: string;
  subpath?: string;
  url?: string;
  label?: string;
  [key: string]: unknown;
}

export interface CanvasEdge {
  id: string;
  fromNode: string;
  toNode: string;
  fromSide?: CanvasSide;
  toSide?: CanvasSide;
  fromEnd?: CanvasEnd;
  toEnd?: CanvasEnd;
  color?: string;
  label?: string;
  [key: string]: unknown;
}

export interface CanvasDocument {
  nodes: CanvasNode[];
  edges: CanvasEdge[];
  [key: string]: unknown;
}

/** Content field each node type carries, per the JSON Canvas spec. */
export const CONTENT_FIELDS: Record<CanvasNodeType, readonly string[]> = {
  text: ['text'],
  file: ['file', 'subpath'],
  link: ['url'],
  group: ['label'],
};

export const DEFAULT_NODE_WIDTH = 400;
export const DEFAULT_NODE_HEIGHT = 200;
const PLACEMENT_GAP = 40;

/** `path` with `.canvas` appended when the caller left it off. */
export function canvasPath(path: string): string {
  return path.endsWith('.canvas') ? path : `${path}.canvas`;
}

/**
 * Parse canvas JSON. An empty file is an empty canvas: Obsidian creates new
 * canvases as zero-byte files. Missing `nodes` / `edges` keys read as empty.
 */
export function parseCanvas(raw: string, relPath: string): CanvasDocument {
  if (raw.trim() === '') return { nodes: [], edges: [] };
  let data: unknown;
  try {
    data = JSON.parse(raw);
  } catch (err) {
    throw new Error(`Canvas ${relPath} is not valid JSON: ${(err as Error).message}`);
  }
  if (data === null || typeof data !== 'object' || Array.isArray(data)) {
    throw new Error(`Canvas ${relPath} is not a JSON object`);
  }
  const doc = data as Record<string, unknown>;
  for (const key of ['nodes', 'edges']) {
    if (doc[key] === undefined) doc[key] = [];
    if (!Array.isArray(doc[key])) {
      throw new Error(`Canvas ${relPath} has a non-array "${key}" field`);
    }
  }
  return doc as CanvasDocument;
}

/**
 * Serialize the way Obsidian does: tab-indented top level, one compact JSON
 * object per node and edge. Keeps diffs of synced vaults to one line per
 * changed element.
 */
export function serializeCanvas(doc: CanvasDocument): string {
  const { nodes, edges, ...rest } = doc;
  const entries: string[] = [`\t"nodes":${serializeArray(nodes)}`, `\t"edges":${serializeArray(edges)}`];
  for (const [key, value] of Object.entries(rest)) {
    entries.push(`\t${JSON.stringify(key)}:${JSON.stringify(value)}`);
  }
  return `{\n${entries.join(',\n')}\n}`;
}

function serializeArray(items: unknown[]): string {
  if (items.length === 0) return '[]';
  return `[\n${items.map((item) => `\t\t${JSON.stringify(item)}`).join(',\n')}\n\t]`;
}

/** Read a canvas; `null` when the file does not exist. */
export async function readCanvas(vaultPath: string, relPath: string): Promise<CanvasDocument | null> {
  const abs = resolveInVault(vaultPath, relPath);
  let raw: string;
  try {
    raw = await fs.readFile(abs, 'utf-8');
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === 'ENOENT') return null;
    throw err;
  }
  return parseCanvas(raw, relPath);
}

/** Write a canvas atomically (temp file + rename), creating parent folders. */
export async function writeCanvas(vaultPath: string, relPath: string, doc: CanvasDocument): Promise<void> {
  const abs = resolveInVault(vaultPath, relPath);
  await fs.mkdir(dirname(abs), { recursive: true });
  const tmp = `${abs}.tmp`;
  await fs.writeFile(tmp, serializeCanvas(doc), 'utf-8');
  await fs.rename(tmp, abs);
}

/** A 16-hex-char id, unique among `doc`'s nodes and edges, like Obsidian's. */
export function generateCanvasId(doc: CanvasDocument): string {
  const taken = new Set<string>([...doc.nodes.map((n) => n.id), ...doc.edges.map((e) => e.id)]);
  let id: string;
  do {
    id = randomBytes(8).toString('hex');
  } while (taken.has(id));
  return id;
}

/** Top-left corner for a new node: right of the right-most node, aligned to its top. */
export function placeRightOf(nodes: CanvasNode[]): { x: number; y: number } {
  let best: { right: number; y: number } | undefined;
  for (const node of nodes) {
    const right = num(node.x) + num(node.width);
    if (!best || right > best.right) best = { right, y: num(node.y) };
  }
  if (!best) return { x: 0, y: 0 };
  return { x: best.right + PLACEMENT_GAP, y: best.y };
}

function num(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0;
}
