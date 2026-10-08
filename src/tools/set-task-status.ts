import { z } from 'zod';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import { runBackgroundReindex } from './background-reindex.js';
import type { ServerContext } from '../context.js';
import { readNoteFile } from './note-file.js';
import { writeFileAtomic } from '../vault/vault-path.js';
import { replaceTaskStatus, scanTasks, taskState } from '../vault/tasks.js';

/**
 * `set_task_status` — rewrite the one character between a task's brackets.
 * The task is looked up with the same scan as `list_tasks`, so a `- [ ]`
 * line inside a code fence or the frontmatter is not a task here either.
 */
export function registerSetTaskStatusTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'set_task_status',
    'Set the status of one markdown task in place. Only the character between the brackets changes; the rest of the file stays byte-identical. Refuses a line that is not a task.',
    {
      name: z.string().describe('Path or fuzzy match of the note.'),
      line: z.number().int().positive().describe('1-based line of the task, as `list_tasks` returns it.'),
      status: z.string().describe('`open` (space), `done` (`x`), or one raw status character such as `/` or `-`.'),
      expectedText: z.string().optional().describe(
        'The task text you expect on that line. The write is refused when it differs, e.g. after the note changed.',
      ),
    },
    async (args) => {
      const status = toStatusChar(args.status);
      const note = await readNoteFile(args.name, ctx);

      const lineCount = note.content.split('\n').length;
      if (args.line > lineCount) {
        throw new Error(`Line ${args.line} is past the end of "${note.path}" (${lineCount} lines).`);
      }
      const task = scanTasks(note.content).find((t) => t.line === args.line);
      if (task === undefined) {
        const text = note.content.split('\n')[args.line - 1]!.replace(/\r$/, '');
        throw new Error(
          `Line ${args.line} of "${note.path}" is not a task: ${JSON.stringify(text)}. Use list_tasks to find the line.`,
        );
      }
      if (args.expectedText !== undefined && args.expectedText.trim() !== task.text.trim()) {
        throw new Error(
          `Task text on line ${args.line} of "${note.path}" is ${JSON.stringify(task.text)}, not ${JSON.stringify(args.expectedText)}. Run list_tasks again and retry.`,
        );
      }

      const changed = task.status !== status;
      if (changed) {
        await writeFileAtomic(note.abs, replaceTaskStatus(note.content, args.line, status));
        runBackgroundReindex(ctx);
      }
      return {
        path: note.path,
        line: args.line,
        previousStatus: task.status,
        status,
        state: taskState(status),
        text: task.text,
        changed,
      };
    },
  );
}

function toStatusChar(status: string): string {
  if (status === 'open') return ' ';
  if (status === 'done') return 'x';
  const chars = [...status];
  if (chars.length !== 1 || /[\r\n[\]]/.test(status)) {
    throw new Error(`status must be "open", "done", or one character other than a bracket or line break; got ${JSON.stringify(status)}`);
  }
  return status;
}
