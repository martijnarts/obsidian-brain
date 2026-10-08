/**
 * Line-level structure of a markdown file: which lines are YAML frontmatter
 * and which sit inside fenced code. Callers split the raw file on `\n`, so a
 * CRLF file keeps a trailing `\r` on each line; every check here ignores it.
 */

const FENCE = /^[ \t]*(`{3,}|~{3,})/;

export function stripCr(line: string): string {
  return line.endsWith('\r') ? line.slice(0, -1) : line;
}

/**
 * 0-based index of the line that closes a leading frontmatter block, or -1
 * when there is none. Frontmatter only counts when the first line is `---`
 * and a later `---` or `...` closes it.
 */
export function frontmatterEnd(lines: string[]): number {
  if (lines.length === 0 || stripCr(lines[0]!).trimEnd() !== '---') return -1;
  for (let i = 1; i < lines.length; i++) {
    const l = stripCr(lines[i]!).trimEnd();
    if (l === '---' || l === '...') return i;
  }
  return -1;
}

/**
 * Inclusive 0-based `[open, close]` line ranges of fenced code blocks. A
 * fence closes on a line of the same character at least as long as the
 * opener; an unclosed fence runs to the end of the file.
 */
export function fenceRanges(lines: string[], from = 0): Array<[number, number]> {
  const ranges: Array<[number, number]> = [];
  let open: { line: number; fence: string } | null = null;
  for (let i = from; i < lines.length; i++) {
    const m = FENCE.exec(stripCr(lines[i]!));
    if (!m) continue;
    const fence = m[1]!;
    if (open === null) {
      open = { line: i, fence };
    } else if (fence[0] === open.fence[0] && fence.length >= open.fence.length) {
      ranges.push([open.line, i]);
      open = null;
    }
  }
  if (open !== null) ranges.push([open.line, lines.length - 1]);
  return ranges;
}

/** Per-line flag: true for frontmatter lines and lines inside a code fence. */
export function nonBodyLines(lines: string[]): boolean[] {
  const skip = new Array<boolean>(lines.length).fill(false);
  const fmEnd = frontmatterEnd(lines);
  for (let i = 0; i <= fmEnd; i++) skip[i] = true;
  for (const [a, b] of fenceRanges(lines, fmEnd + 1)) {
    for (let i = a; i <= b; i++) skip[i] = true;
  }
  return skip;
}
