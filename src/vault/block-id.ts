/**
 * `^block-id` placement, following Obsidian's rules: a paragraph takes
 * ` ^id` at the end of its last line, a list item or heading on its own
 * line, and a table, quote, callout or fenced code block takes `^id` on a
 * separate line after the block, set off by blank lines. Pure functions over
 * the file split on `\n`; the caller owns the read and the write.
 */

import { randomBytes } from 'crypto';
import { fenceRanges, frontmatterEnd, stripCr } from './markdown-lines.js';

export const BLOCK_ID = /^[A-Za-z0-9-]+$/;
const TRAILING_ID = /(?:^|\s)\^([A-Za-z0-9-]+)$/;
const OWN_LINE_ID = /^\^([A-Za-z0-9-]+)$/;
const HEADING = /^#{1,6}(?:[ \t]|$)/;
const LIST_ITEM = /^[ \t]*(?:[-*+]|\d+[.)])(?:[ \t]|$)/;
const TABLE_ROW = /^[ \t]*\|/;
const QUOTE = /^[ \t]*>/;

export type Block =
  | { kind: 'inline'; start: number; end: number }
  | { kind: 'own-line'; start: number; end: number };

const isBlank = (l: string): boolean => stripCr(l).trim() === '';
const clean = (l: string): string => stripCr(l).trimEnd();

/** The block that 0-based line `idx` belongs to. Throws for lines that cannot carry an id. */
export function locateBlock(lines: string[], idx: number): Block {
  if (idx <= frontmatterEnd(lines)) {
    throw new Error(`Line ${idx + 1} is in the frontmatter; a block id needs a body line`);
  }
  if (isBlank(lines[idx]!)) throw new Error(`Line ${idx + 1} is blank; a block id needs a line with content`);

  const fence = fenceRanges(lines, frontmatterEnd(lines) + 1).find(([a, b]) => idx >= a && idx <= b);
  if (fence) return { kind: 'own-line', start: fence[0], end: fence[1] };

  const text = clean(lines[idx]!);
  if (HEADING.test(text) || LIST_ITEM.test(text)) return { kind: 'inline', start: idx, end: idx };

  const extent = (test: (l: string) => boolean): [number, number] => {
    let a = idx;
    let b = idx;
    while (a > 0 && test(clean(lines[a - 1]!))) a--;
    while (b + 1 < lines.length && test(clean(lines[b + 1]!))) b++;
    return [a, b];
  };
  for (const re of [TABLE_ROW, QUOTE]) {
    if (re.test(text)) {
      const [a, b] = extent((l) => re.test(l));
      return { kind: 'own-line', start: a, end: b };
    }
  }
  const plain = (l: string): boolean =>
    l.trim() !== '' &&
    ![HEADING, LIST_ITEM, TABLE_ROW, QUOTE, OWN_LINE_ID].some((re) => re.test(l)) &&
    !/^[ \t]*(`{3,}|~{3,})/.test(l);
  const [a, b] = extent(plain);
  return { kind: 'inline', start: a, end: b };
}

/** The id the block already carries, with its 0-based line. */
export function existingBlockId(lines: string[], block: Block): { id: string; line: number } | null {
  if (block.kind === 'inline') {
    const m = TRAILING_ID.exec(clean(lines[block.end]!));
    return m ? { id: m[1]!, line: block.end } : null;
  }
  let i = block.end + 1;
  if (i < lines.length && isBlank(lines[i]!)) i++;
  const m = i < lines.length ? OWN_LINE_ID.exec(clean(lines[i]!)) : null;
  return m ? { id: m[1]!, line: i } : null;
}

/** Every block id written in the note. */
export function allBlockIds(lines: string[]): Set<string> {
  const ids = new Set<string>();
  for (const l of lines) {
    const m = TRAILING_ID.exec(clean(l)) ?? OWN_LINE_ID.exec(clean(l));
    if (m) ids.add(m[1]!);
  }
  return ids;
}

/** New lines with `id` attached to `block`, and the 0-based line the id sits on. */
export function attachBlockId(lines: string[], block: Block, id: string): { lines: string[]; line: number } {
  const out = [...lines];
  if (block.kind === 'inline') {
    const l = out[block.end]!;
    out[block.end] = `${clean(l)} ^${id}${l.endsWith('\r') ? '\r' : ''}`;
    return { lines: out, line: block.end };
  }

  const eol = lines.some((l) => l.endsWith('\r')) ? '\r' : '';
  const after = block.end + 1;
  const hasNext = after < out.length;
  // The block may be the last line of a file with no final newline; it now
  // gets a successor, so it needs the line ending the others carry.
  if (eol && !out[block.end]!.endsWith('\r')) out[block.end] += eol;
  const blankAfter = hasNext && !isBlank(out[after]!) ? [eol] : [];
  out.splice(after, 0, eol, `^${id}${hasNext ? eol : ''}`, ...blankAfter);
  return { lines: out, line: after + 1 };
}

/** A 6-character lowercase id not in `taken`. */
export function generateBlockId(taken: ReadonlySet<string>): string {
  const alphabet = 'abcdefghijklmnopqrstuvwxyz0123456789';
  for (;;) {
    const id = Array.from(randomBytes(6), (b) => alphabet[b % alphabet.length]).join('');
    if (!taken.has(id)) return id;
  }
}

/** 0-based lines of body headings whose text is `heading` (leading `#`s optional). */
export function findHeadingLines(lines: string[], heading: string): number[] {
  const want = headingText(heading);
  const fmEnd = frontmatterEnd(lines);
  const fenced = fenceRanges(lines, fmEnd + 1);
  const found: number[] = [];
  for (let i = fmEnd + 1; i < lines.length; i++) {
    const l = clean(lines[i]!);
    if (!HEADING.test(l)) continue;
    if (fenced.some(([a, b]) => i >= a && i <= b)) continue;
    if (headingText(l) === want) found.push(i);
  }
  return found;
}

function headingText(h: string): string {
  return h
    .trim()
    .replace(/^#{1,6}(?:[ \t]+|$)/, '')
    .replace(/(?:^|\s)\^[A-Za-z0-9-]+$/, '')
    .replace(/[ \t]+#+$/, '')
    .trim();
}
