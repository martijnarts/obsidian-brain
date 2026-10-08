import { describe, it, expect } from 'vitest';
import { replaceTaskStatus, scanTasks } from '../../src/vault/tasks.js';
import { fenceRanges, frontmatterEnd, nonBodyLines } from '../../src/vault/markdown-lines.js';

describe('markdown-lines', () => {
  it('finds closed frontmatter only', () => {
    expect(frontmatterEnd(['---', 'a: 1', '...', 'body'])).toBe(2);
    expect(frontmatterEnd(['---\r', 'a: 1\r', '---\r'])).toBe(2);
    expect(frontmatterEnd(['---', 'never closed'])).toBe(-1);
    expect(frontmatterEnd([])).toBe(-1);
  });

  it('pairs fences by character and length, and runs an unclosed one to the end', () => {
    expect(fenceRanges(['````', '```', '````', 'x', '~~~', 'y'])).toEqual([[0, 2], [4, 5]]);
    expect(nonBodyLines(['---', 'k: v', '---', '```', 'code', '```', 'text']))
      .toEqual([true, true, true, true, true, true, false]);
  });
});

describe('scanTasks', () => {
  it('skips tasks in an unclosed fence and treats unclosed frontmatter as body', () => {
    expect(scanTasks('---\n- [ ] body task\n```\n- [ ] code').map((t) => t.line)).toEqual([2]);
  });

  it('resets nesting after a flush-left paragraph and keeps it over indented text', () => {
    const tasks = scanTasks('- [ ] a\n  continued\n  - [ ] b\nPara\n  - [ ] c');
    expect(tasks.map((t) => [t.line, t.indent, t.parentLine])).toEqual([
      [1, 0, undefined],
      [3, 1, 1],
      [5, 0, undefined],
    ]);
  });

  it('accepts + markers and 1) numbering', () => {
    expect(scanTasks('+ [ ] plus\n2) [x] paren').map((t) => t.state)).toEqual(['open', 'done']);
  });
});

describe('replaceTaskStatus', () => {
  it('changes one character and throws on a non-task line', () => {
    expect(replaceTaskStatus('a\n- [ ] b\nc', 2, 'x')).toBe('a\n- [x] b\nc');
    expect(replaceTaskStatus('- [🔥] hot', 1, ' ')).toBe('- [ ] hot');
    expect(() => replaceTaskStatus('plain', 1, 'x')).toThrow(/not a task/);
  });
});
