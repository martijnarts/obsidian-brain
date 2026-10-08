/**
 * Locate parts of a raw note: its headings, the section under one heading,
 * the block that carries a `^blockId`, a line range, and its tasks.
 *
 * Every function takes the whole file, frontmatter included, so the 1-based
 * line numbers it reports match what an editor shows. Frontmatter and fenced
 * code blocks never yield headings, blocks or tasks.
 */

export interface Heading {
  level: number;
  text: string;
  /** 1-based line number in the file. */
  line: number;
}

export interface NotePart {
  startLine: number;
  endLine: number;
  content: string;
}

const HEADING_RE = /^ {0,3}(#{1,6})[ \t]+(.+?)(?:[ \t]+#+)?[ \t]*$/;
const FENCE_RE = /^ {0,3}(`{3,}|~{3,})/;
const LIST_ITEM_RE = /^\s*(?:[-*+]|\d+[.)])\s/;
const TASK_RE = /^\s*(?:[-*+]|\d+[.)])\s+\[(.)\]/;
const BLOCK_ID_RE = /(?:^|\s)\^([A-Za-z0-9-]+)\s*$/;

/** Split into lines; a final newline does not add an empty last line. */
export function splitLines(raw: string): string[] {
  const lines = raw.split(/\r?\n/);
  if (lines.length > 1 && lines[lines.length - 1] === '') lines.pop();
  return lines;
}

/**
 * For each line, true when it is frontmatter, a code fence delimiter or
 * inside a fenced code block: lines that carry no markdown structure.
 */
function inertLines(lines: string[]): boolean[] {
  const inert = new Array<boolean>(lines.length).fill(false);
  let start = 0;
  if (lines[0]?.trimEnd() === '---') {
    const close = lines.findIndex((l, i) => i > 0 && (l.trimEnd() === '---' || l.trimEnd() === '...'));
    if (close !== -1) {
      for (let i = 0; i <= close; i++) inert[i] = true;
      start = close + 1;
    }
  }
  let fence: string | null = null;
  for (let i = start; i < lines.length; i++) {
    const m = FENCE_RE.exec(lines[i]!);
    if (fence === null) {
      if (m) {
        fence = m[1]!;
        inert[i] = true;
      }
      continue;
    }
    inert[i] = true;
    if (m && m[1]![0] === fence[0] && m[1]!.length >= fence.length && lines[i]!.trim() === m[1]) {
      fence = null;
    }
  }
  return inert;
}

export function extractHeadings(raw: string): Heading[] {
  const lines = splitLines(raw);
  const inert = inertLines(lines);
  const headings: Heading[] = [];
  lines.forEach((line, i) => {
    if (inert[i]) return;
    const m = HEADING_RE.exec(line);
    if (m) headings.push({ level: m[1]!.length, text: m[2]!.trim(), line: i + 1 });
  });
  return headings;
}

export function countTasks(raw: string): { open: number; done: number } {
  const lines = splitLines(raw);
  const inert = inertLines(lines);
  let open = 0;
  let done = 0;
  lines.forEach((line, i) => {
    if (inert[i]) return;
    const m = TASK_RE.exec(line);
    if (!m) return;
    if (m[1] === ' ') open++;
    else done++;
  });
  return { open, done };
}

/**
 * The section under a heading: the heading line down to the line before the
 * next heading of the same or a higher level, or the end of the file.
 *
 * `query` matches a heading's text exactly, then case-insensitively. When
 * neither matches and `query` holds `>`, it is read as a nested path such as
 * `Project > Notes`, where each segment must sit under the one before it.
 */
export function findHeadingSection(raw: string, query: string): NotePart & { heading: Heading } {
  const lines = splitLines(raw);
  const headings = extractHeadings(raw);
  const sectionEnd = (h: Heading): number => {
    const next = headings.find((o) => o.line > h.line && o.level <= h.level);
    return next ? next.line - 1 : lines.length;
  };

  let heading = pickHeading(headings, query.trim(), 'the note');
  if (heading === undefined && query.includes('>')) {
    const segments = query.split('>').map((s) => s.trim()).filter((s) => s.length > 0);
    let scope: Heading[] = headings;
    let scopeName = 'the note';
    for (const segment of segments) {
      const found = pickHeading(scope, segment, scopeName);
      if (found === undefined) {
        throw new Error(`Heading "${segment}" not found under ${scopeName}`);
      }
      heading = found;
      const end = sectionEnd(found);
      scope = headings.filter((h) => h.line > found.line && h.line <= end && h.level > found.level);
      scopeName = `"${found.text}"`;
    }
  }
  if (heading === undefined) {
    throw new Error(`Heading "${query}" not found`);
  }

  const endLine = sectionEnd(heading);
  return {
    heading,
    startLine: heading.line,
    endLine,
    content: lines.slice(heading.line - 1, endLine).join('\n'),
  };
}

function pickHeading(candidates: Heading[], text: string, scopeName: string): Heading | undefined {
  let hits = candidates.filter((h) => h.text === text);
  if (hits.length === 0) {
    const lower = text.toLowerCase();
    hits = candidates.filter((h) => h.text.toLowerCase() === lower);
  }
  if (hits.length > 1) {
    const where = hits.map((h) => `line ${h.line}`).join(', ');
    throw new Error(
      `Heading "${text}" is ambiguous in ${scopeName} (${where}). Use a nested path such as "Parent > ${text}".`,
    );
  }
  return hits[0];
}

/**
 * The block that carries `^blockId`: the list item it ends, or the paragraph
 * it ends. A block id alone on its own line marks the block above it (a
 * table, a quote), and that block includes the id line.
 */
export function findBlock(raw: string, blockId: string): NotePart & { blockId: string } {
  const id = blockId.trim().replace(/^\^+/, '');
  if (id === '') throw new Error('blockId is empty');
  const lines = splitLines(raw);
  const inert = inertLines(lines);

  const at = lines.findIndex((line, i) => !inert[i] && BLOCK_ID_RE.exec(line)?.[1] === id);
  if (at === -1) throw new Error(`Block "^${id}" not found`);

  const isBoundary = (i: number): boolean =>
    inert[i]! || lines[i]!.trim() === '' || HEADING_RE.test(lines[i]!);

  let start = at;
  if (lines[at]!.trim() === `^${id}`) {
    let i = at - 1;
    while (i >= 0 && lines[i]!.trim() === '' && !inert[i]) i--;
    while (i >= 0 && !isBoundary(i)) {
      start = i;
      i--;
    }
  } else if (!LIST_ITEM_RE.test(lines[at]!)) {
    while (start > 0 && !isBoundary(start - 1) && !LIST_ITEM_RE.test(lines[start - 1]!)) start--;
  }

  return {
    blockId: id,
    startLine: start + 1,
    endLine: at + 1,
    content: lines.slice(start, at + 1).join('\n'),
  };
}

/** Lines `startLine`..`endLine`, 1-based and inclusive; `endLine` is clamped to the file. */
export function sliceLines(
  raw: string,
  startLine: number,
  endLine?: number,
): NotePart & { totalLines: number } {
  const lines = splitLines(raw);
  const total = lines.length;
  if (startLine > total) {
    throw new Error(`startLine ${startLine} is past the end of the note (${total} lines)`);
  }
  const end = Math.min(endLine ?? total, total);
  if (end < startLine) {
    throw new Error(`endLine ${endLine} is before startLine ${startLine}`);
  }
  return {
    startLine,
    endLine: end,
    totalLines: total,
    content: lines.slice(startLine - 1, end).join('\n'),
  };
}
