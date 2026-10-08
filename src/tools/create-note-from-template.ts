import { z } from 'zod';
import { existsSync, readFileSync } from 'node:fs';
import { basename } from 'node:path';
import matter from 'gray-matter';
import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { registerTool } from './register.js';
import { runBackgroundReindex } from './background-reindex.js';
import type { ServerContext } from '../context.js';
import { migrateStubToReal } from '../store/nodes.js';
import { resolveInVault } from '../vault/paths.js';
import {
  BUILTIN_VARIABLES,
  findTemplate,
  readTemplatesConfig,
  renderTemplate,
} from '../vault/templates.js';

const TEMPLATER_SYNTAX = /<%[\s\S]*?%>/;

/**
 * `create_note_from_template` — render an Obsidian core Templates template
 * and create the note through the same `VaultWriter` path as `create_note`,
 * so frontmatter handling and indexing match.
 */
export function registerCreateNoteFromTemplateTool(server: McpServer, ctx: ServerContext): void {
  registerTool(
    server,
    'create_note_from_template',
    'Create a note from an Obsidian core Templates template, filling `{{title}}`, `{{date}}`, `{{time}}`, `{{date:FORMAT}}`, `{{time:FORMAT}}` and any `variables`. Templater `<% %>` code is not run and stays as written. Fails if the note exists.',
    {
      template: z.string().min(1).describe('Template name in the templates folder, or a vault-relative path. `.md` is optional.'),
      title: z.string().min(1).describe('Note title. Used as the filename base and for `{{title}}`.'),
      directory: z.string().optional().describe('Vault-relative subdirectory to create the note in.'),
      variables: z.record(z.string(), z.string()).optional().describe('Extra `{{key}}` substitutions.'),
      date: z.string().optional().describe('ISO date or date-time to use instead of now.'),
    },
    async (args) => {
      const { template, title, variables } = args;
      const directory = args.directory?.replace(/^\/+|\/+$/g, '') || undefined;

      if (/[\\/]/.test(title)) {
        throw new Error('Title cannot contain "/" or "\\"; use `directory` for folders');
      }
      const reserved = Object.keys(variables ?? {}).filter((k) =>
        (BUILTIN_VARIABLES as readonly string[]).includes(k.toLowerCase()),
      );
      if (reserved.length > 0) {
        throw new Error(`\`variables\` cannot set ${reserved.join(', ')}; these are filled from the arguments`);
      }
      const now = parseDate(args.date);

      const target = directory ? `${directory}/${title}.md` : `${title}.md`;
      if (existsSync(resolveInVault(ctx.config.vaultPath, target))) {
        throw new Error(`File already exists: ${target}`);
      }

      const config = readTemplatesConfig(ctx.config.vaultPath);
      const templatePath = findTemplate(ctx.config.vaultPath, template, config);
      const raw = readFileSync(resolveInVault(ctx.config.vaultPath, templatePath), 'utf-8');
      const rendered = renderTemplate(raw, { title, now, config, variables });

      let parsed: matter.GrayMatterFile<string>;
      try {
        parsed = matter(rendered.content);
      } catch (err) {
        throw new Error(`Template ${templatePath} has invalid YAML frontmatter: ${(err as Error).message}`);
      }

      const path = ctx.writer.createNode({
        title,
        content: parsed.content,
        directory,
        frontmatter: { ...parsed.data },
      });

      // Same forward-reference stub migration as create_note.
      const stem = basename(path, '.md');
      if (stem) {
        migrateStubToReal(ctx.db, `_stub/${stem}.md`, path);
      }

      runBackgroundReindex(ctx);

      return {
        path,
        title,
        template: templatePath,
        ...(rendered.unresolved.length > 0 ? { unresolved: rendered.unresolved } : {}),
        ...(TEMPLATER_SYNTAX.test(raw)
          ? { templater: 'Templater syntax (<% %>) was left as written; this server does not run Templater.' }
          : {}),
      };
    },
  );
}

/** A date-only ISO string means local midnight, not UTC midnight. */
function parseDate(value: string | undefined): Date {
  if (value === undefined) return new Date();
  const dateOnly = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  const date = dateOnly
    ? new Date(Number(dateOnly[1]), Number(dateOnly[2]) - 1, Number(dateOnly[3]))
    : new Date(value);
  if (Number.isNaN(date.getTime())) throw new Error(`Invalid date: ${value}`);
  return date;
}
