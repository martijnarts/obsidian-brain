/**
 * Markdown task lines (`- [ ] text`, `* [x] text`, `1. [/] text`) read
 * straight from file content, without Obsidian's metadata cache.
 */

import { nonBodyLines, stripCr } from './markdown-lines.js';

const TASK = /^([ \t]*)([-*+]|\d+[.)])([ \t]+)\[(.)\](?:[ \t]+(.*))?$/u;
const LIST_ITEM = /^([ \t]*)(?:[-*+]|\d+[.)])(?:[ \t]|$)/;
const DUE = /📅\s*(\d{4}-\d{2}-\d{2})/u;
const TAB_WIDTH = 4;

export type TaskState = 'open' | 'done' | 'cancelled';

export interface Task {
  /** 1-based line number, counted from the start of the file. */
  line: number;
  /** The raw character between the brackets. */
  status: string;
  state: TaskState;
  text: string;
  /** Nesting depth among list items: 0 for a top-level item. */
  indent: number;
  /** Line of the nearest enclosing task, when the task is nested under one. */
  parentLine?: number;
  /** Obsidian Tasks due date (`📅 YYYY-MM-DD`). */
  due?: string;
}

export interface TaskLine {
  /** Leading whitespace, list marker and the space before `[`. */
  prefix: string;
  status: string;
  text: string;
}

export function parseTaskLine(line: string): TaskLine | null {
  const m = TASK.exec(stripCr(line));
  if (!m) return null;
  return { prefix: m[1]! + m[2]! + m[3]!, status: m[4]!, text: (m[5] ?? '').trimEnd() };
}

export function taskState(status: string): TaskState {
  if (status === 'x' || status === 'X') return 'done';
  if (status === '-') return 'cancelled';
  return 'open';
}

function indentWidth(ws: string): number {
  let w = 0;
  for (const ch of ws) w += ch === '\t' ? TAB_WIDTH : 1;
  return w;
}

/** Every task in `content`, skipping frontmatter and fenced code. */
export function scanTasks(content: string): Task[] {
  const lines = content.split('\n');
  const skip = nonBodyLines(lines);
  const tasks: Task[] = [];
  // Open list items above the current line, outermost first.
  const stack: Array<{ width: number; line: number; task: boolean }> = [];

  for (let i = 0; i < lines.length; i++) {
    if (skip[i]) continue;
    const raw = stripCr(lines[i]!);
    const item = LIST_ITEM.exec(raw);
    if (!item) {
      // A flush-left line that is not a list item ends the list; blank and
      // indented continuation lines keep it open.
      if (raw.trim() !== '' && !/^[ \t]/.test(raw)) stack.length = 0;
      continue;
    }

    const width = indentWidth(item[1]!);
    while (stack.length > 0 && stack[stack.length - 1]!.width >= width) stack.pop();
    const parent = [...stack].reverse().find((s) => s.task);
    const parsed = parseTaskLine(raw);
    stack.push({ width, line: i + 1, task: parsed !== null });
    if (parsed === null) continue;

    const due = DUE.exec(parsed.text)?.[1];
    tasks.push({
      line: i + 1,
      status: parsed.status,
      state: taskState(parsed.status),
      text: parsed.text,
      indent: stack.length - 1,
      ...(parent !== undefined ? { parentLine: parent.line } : {}),
      ...(due !== undefined ? { due } : {}),
    });
  }
  return tasks;
}

/**
 * `content` with the status character of the task on `line` (1-based)
 * replaced. Every other byte stays as it was.
 */
export function replaceTaskStatus(content: string, line: number, status: string): string {
  let start = 0;
  for (let i = 1; i < line; i++) start = content.indexOf('\n', start) + 1;
  const end = content.indexOf('\n', start);
  const text = content.slice(start, end === -1 ? content.length : end);
  const parsed = parseTaskLine(text);
  if (parsed === null) throw new Error(`Line ${line} is not a task`);
  const at = start + parsed.prefix.length + 1;
  return content.slice(0, at) + status + content.slice(at + parsed.status.length);
}
