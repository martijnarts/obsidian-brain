/**
 * Helpers shared by the vault maintenance tools (`find_broken_links`,
 * `find_orphaned_notes`, `search_and_replace`, `rename_tag`,
 * `rename_heading`). They work on raw file text so a rewrite can keep every
 * byte it does not mean to change.
 */

import { promises as fs } from 'fs';
import { resolve, sep } from 'path';
import type { DatabaseHandle } from '../store/db.js';
import { errorMessage } from '../util/errors.js';

/**
 * Normalise a client-supplied vault folder: forward slashes, no leading or
 * trailing slash. `''` means the vault root.
 */
export function normalizeFolder(folder: string): string {
  return folder.replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
}

export function isUnderFolder(id: string, folder: string): boolean {
  return folder === '' || id.startsWith(`${folder}/`);
}

/**
 * Every indexed real note (stubs excluded) under `folder` and outside every
 * `excludeFolders` entry, sorted by path.
 */
export function listNotePaths(
  db: DatabaseHandle,
  folder?: string,
  excludeFolders?: string[],
): string[] {
  const root = normalizeFolder(folder ?? '');
  const excluded = (excludeFolders ?? []).map(normalizeFolder).filter((f) => f !== '');
  const rows = db
    .prepare("SELECT id FROM nodes WHERE id NOT LIKE '\\_stub/%' ESCAPE '\\' ORDER BY id")
    .all() as Array<{ id: string }>;
  return rows
    .map((r) => r.id)
    .filter((id) => isUnderFolder(id, root) && !excluded.some((f) => isUnderFolder(id, f)));
}

/**
 * Absolute path of a vault-relative file, refusing anything that resolves
 * outside the vault: `..` segments, absolute paths, and symlinks whose real
 * target lies elsewhere.
 */
export async function vaultFilePath(vaultPath: string, relPath: string): Promise<string> {
  const root = await fs.realpath(vaultPath);
  const abs = resolve(root, relPath);
  if (abs !== root && !abs.startsWith(root + sep)) {
    throw new Error(`Path escapes the vault: ${relPath}`);
  }
  const real = await fs.realpath(abs);
  if (!real.startsWith(root + sep)) {
    throw new Error(`Path escapes the vault: ${relPath}`);
  }
  return abs;
}

/** Read a note, or `null` when it is missing or lies outside the vault. */
export async function readNoteFile(vaultPath: string, relPath: string): Promise<string | null> {
  try {
    return await fs.readFile(await vaultFilePath(vaultPath, relPath), 'utf-8');
  } catch {
    return null;
  }
}

/** Write through a temp file and rename, so a reader never sees half a file. */
export async function writeNoteFile(vaultPath: string, relPath: string, content: string): Promise<void> {
  const abs = await vaultFilePath(vaultPath, relPath);
  const tmp = `${abs}.tmp`;
  await fs.writeFile(tmp, content, 'utf-8');
  await fs.rename(tmp, abs);
}

/** Split a file into its YAML frontmatter block (fences included) and body. */
export function splitFrontmatter(raw: string): { frontmatter: string; body: string } {
  const m = /^---\r?\n(?:[\s\S]*?\r?\n)?---[ \t]*(?:\r?\n|$)/.exec(raw);
  if (!m) return { frontmatter: '', body: raw };
  return { frontmatter: m[0], body: raw.slice(m[0].length) };
}

function blank(s: string): string {
  return s.replace(/[^\n]/g, ' ');
}

// An inline code span: a backtick run closed by a run of the same length,
// never across a blank line.
const INLINE_CODE = /(?<!`)(`+)(?!`)(?:(?!\n[ \t]*\n)[\s\S])*?(?<!`)\1(?!`)/g;
// A fenced block runs to its closing fence, or to the end of the text when
// it is never closed.
const FENCE = /^( {0,3})(`{3,}|~{3,})[^\n]*\n[\s\S]*?(?:^ {0,3}\2[`~]*[ \t]*$|(?![\s\S]))/gm;

/** `[start, end)` offsets of every fenced code block, fences included. */
export function fencedCodeRanges(text: string): Array<[number, number]> {
  return [...text.matchAll(FENCE)].map((m) => [m.index, m.index + m[0].length]);
}

/**
 * Same-length copy of `text` with fenced code blocks (and, unless
 * `fencedOnly`, inline code spans) blanked to spaces. Offsets and line
 * numbers in the result match the original, so a match found in the mask
 * can be spliced into the original text.
 */
export function maskCode(text: string, fencedOnly = false): string {
  const masked = text.replace(FENCE, blank);
  if (fencedOnly) return masked;
  return masked.replace(INLINE_CODE, blank);
}

/** 1-based line number of an offset. */
export function lineAt(text: string, offset: number): number {
  let line = 1;
  for (let i = text.indexOf('\n'); i !== -1 && i < offset; i = text.indexOf('\n', i + 1)) line++;
  return line;
}

export interface Heading {
  level: number;
  text: string;
  /** Offset of the heading text within the scanned string. */
  start: number;
  end: number;
  /** 1-based line number. */
  line: number;
}

const HEADING = /^(#{1,6})[ \t]+(.*?)(?:[ \t]+#+)?[ \t]*$/gm;

/** ATX headings of a note body, outside fenced code. */
export function findHeadings(body: string): Heading[] {
  const masked = maskCode(body, true);
  const out: Heading[] = [];
  for (const m of masked.matchAll(HEADING)) {
    const text = m[2]!;
    if (text === '') continue;
    const start = m.index + m[0].indexOf(text, m[1]!.length);
    out.push({ level: m[1]!.length, text, start, end: start + text.length, line: lineAt(body, m.index) });
  }
  return out;
}

/** Block ids (`^id` at the end of a line) of a note body, outside fenced code. */
export function findBlockIds(body: string): Set<string> {
  const ids = new Set<string>();
  for (const m of maskCode(body, true).matchAll(/(?:^|\s)\^([A-Za-z0-9-]+)[ \t]*$/gm)) {
    ids.add(m[1]!);
  }
  return ids;
}

/** Heading comparison key: Obsidian resolves heading links case-insensitively. */
export function headingKey(text: string): string {
  return text.trim().replace(/\s+/g, ' ').toLowerCase();
}

/**
 * A wiki-link or embed in raw text. `target` is the note part, `subpath`
 * the part after the first `#` (null when absent), so a block reference
 * reads `^id`.
 */
export interface WikiLinkMatch {
  embed: boolean;
  target: string;
  subpath: string | null;
  alias: string | null;
  start: number;
  end: number;
  raw: string;
}

/** Wiki-links and embeds outside fenced and inline code. */
export function findWikiLinks(text: string): WikiLinkMatch[] {
  const out: WikiLinkMatch[] = [];
  const masked = maskCode(text);
  for (const m of masked.matchAll(/(!?)\[\[([^\]\n]+)\]\]/g)) {
    const inner = m[2]!;
    const pipe = inner.indexOf('|');
    const link = pipe === -1 ? inner : inner.slice(0, pipe);
    // `[[Note^id]]` is shorthand for `[[Note#^id]]`, as in the parser.
    const cut = link.search(/[#^]/);
    out.push({
      embed: m[1] === '!',
      target: (cut === -1 ? link : link.slice(0, cut)).trim(),
      subpath: cut === -1 ? null : link.slice(link[cut] === '#' ? cut + 1 : cut),
      alias: pipe === -1 ? null : inner.slice(pipe + 1),
      start: m.index,
      end: m.index + m[0].length,
      raw: text.slice(m.index, m.index + m[0].length),
    });
  }
  return out;
}

const MAX_PATTERN_LENGTH = 500;
const NESTED_QUANTIFIER = /\((?:[^()\\]|\\.)*(?:[+*]|\{\d+,?\d*\})(?:[^()\\]|\\.)*\)(?:[+*]|\{\d+,?\d*\})/;

/**
 * Compile a client regex with the guards every pattern tool applies: a
 * length cap, and no quantified group that itself holds a quantifier (the
 * classic catastrophic-backtracking shape). `literal` escapes the pattern
 * so it matches as plain text.
 */
export function compileUserRegex(pattern: string, flags: string, literal = false): RegExp {
  if (pattern.length > MAX_PATTERN_LENGTH) {
    throw new Error(`Pattern is longer than ${MAX_PATTERN_LENGTH} characters`);
  }
  if (literal) return new RegExp(pattern.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), flags);
  if (NESTED_QUANTIFIER.test(pattern)) {
    throw new Error('Pattern has a nested quantifier, which can backtrack catastrophically. Simplify it.');
  }
  try {
    return new RegExp(pattern, flags);
  } catch (err) {
    throw new Error(`Invalid regex: ${errorMessage(err)}`);
  }
}
