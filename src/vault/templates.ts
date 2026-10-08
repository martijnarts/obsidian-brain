/**
 * Obsidian core Templates, rendered headless: finds a template in the
 * folder `.obsidian/templates.json` names and fills `{{title}}`,
 * `{{date}}`, `{{time}}` and their `:FORMAT` variants the way the core
 * plugin does. Templater (`<% %>`) is a separate plugin and is not run.
 */

import { existsSync, readFileSync, statSync } from 'fs';
import { join } from 'path';
import { resolveVaultPath, toVaultRelative } from './vault-path.js';

export interface TemplatesConfig {
  folder: string;
  dateFormat: string;
  timeFormat: string;
}

export const DEFAULT_TEMPLATES_FOLDER = 'Templates';
export const DEFAULT_DATE_FORMAT = 'YYYY-MM-DD';
export const DEFAULT_TIME_FORMAT = 'HH:mm';

/** Names the core plugin fills itself; `variables` may not reuse them. */
export const BUILTIN_VARIABLES = ['title', 'date', 'time'] as const;

/** Settings from `.obsidian/templates.json`, with Obsidian's defaults for anything unset. */
export function readTemplatesConfig(vaultPath: string): TemplatesConfig {
  let raw: Record<string, unknown> = {};
  try {
    const parsed: unknown = JSON.parse(readFileSync(join(vaultPath, '.obsidian', 'templates.json'), 'utf-8'));
    if (parsed && typeof parsed === 'object') raw = parsed as Record<string, unknown>;
  } catch {
    // Missing or unreadable settings: the plugin's defaults apply.
  }
  const str = (value: unknown): string | undefined =>
    typeof value === 'string' && value.trim() !== '' ? value : undefined;
  return {
    folder: (str(raw.folder) ?? DEFAULT_TEMPLATES_FOLDER).replace(/^\/+|\/+$/g, ''),
    dateFormat: str(raw.dateFormat) ?? DEFAULT_DATE_FORMAT,
    timeFormat: str(raw.timeFormat) ?? DEFAULT_TIME_FORMAT,
  };
}

/**
 * Find a template by vault-relative path or by name inside the templates
 * folder, with or without `.md`. Returns the vault-relative path.
 */
export function findTemplate(vaultPath: string, template: string, config: TemplatesConfig): string {
  const names = template.endsWith('.md') ? [template] : [template, `${template}.md`];
  const candidates = [
    ...names.map((n) => (config.folder ? `${config.folder}/${n}` : n)),
    ...names,
  ];
  for (const candidate of candidates) {
    const { abs } = resolveVaultPath(vaultPath, candidate);
    if (existsSync(abs) && statSync(abs).isFile()) return toVaultRelative(vaultPath, abs);
  }
  throw new Error(`Template not found: "${template}" (looked in "${config.folder || '/'}" and the vault root)`);
}

export interface RenderOptions {
  title: string;
  now: Date;
  config: TemplatesConfig;
  variables?: Record<string, string>;
}

export interface RenderResult {
  content: string;
  /** `{{name}}` placeholders left in place because nothing filled them. */
  unresolved: string[];
}

/** Fill a template's placeholders. Unknown `{{name}}` placeholders stay as written. */
export function renderTemplate(template: string, opts: RenderOptions): RenderResult {
  const unresolved = new Set<string>();
  const content = template.replace(/{{\s*([^{}:]+?)\s*(?::([^{}]*))?}}/g, (match, rawName: string, format?: string) => {
    const name = rawName.toLowerCase();
    if (name === 'title' && format === undefined) return opts.title;
    if (name === 'date') return formatDate(opts.now, format?.trim() || opts.config.dateFormat);
    if (name === 'time') return formatDate(opts.now, format?.trim() || opts.config.timeFormat);
    if (format === undefined && opts.variables && Object.hasOwn(opts.variables, rawName)) {
      return opts.variables[rawName]!;
    }
    unresolved.add(match);
    return match;
  });
  return { content, unresolved: [...unresolved] };
}

const MONTHS = [
  'January', 'February', 'March', 'April', 'May', 'June',
  'July', 'August', 'September', 'October', 'November', 'December',
];
const WEEKDAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];

const TOKEN = /\[([^\]]*)]|YYYY|YY|MMMM|MMM|MM|M|Do|DD|D|dddd|ddd|dd|d|HH|H|hh|h|mm|m|ss|s|A|a/g;

/**
 * A small subset of moment.js formatting, in local time and English:
 * years, months, days of month and week, 12/24-hour clock, minutes,
 * seconds, AM/PM, and `[literal]` text. Other characters pass through.
 */
export function formatDate(date: Date, format: string): string {
  const pad = (n: number): string => String(n).padStart(2, '0');
  const hours12 = date.getHours() % 12 || 12;
  return format.replace(TOKEN, (token, literal?: string) => {
    if (literal !== undefined) return literal;
    switch (token) {
      case 'YYYY': return String(date.getFullYear()).padStart(4, '0');
      case 'YY': return pad(date.getFullYear() % 100);
      case 'MMMM': return MONTHS[date.getMonth()]!;
      case 'MMM': return MONTHS[date.getMonth()]!.slice(0, 3);
      case 'MM': return pad(date.getMonth() + 1);
      case 'M': return String(date.getMonth() + 1);
      case 'Do': return ordinal(date.getDate());
      case 'DD': return pad(date.getDate());
      case 'D': return String(date.getDate());
      case 'dddd': return WEEKDAYS[date.getDay()]!;
      case 'ddd': return WEEKDAYS[date.getDay()]!.slice(0, 3);
      case 'dd': return WEEKDAYS[date.getDay()]!.slice(0, 2);
      case 'd': return String(date.getDay());
      case 'HH': return pad(date.getHours());
      case 'H': return String(date.getHours());
      case 'hh': return pad(hours12);
      case 'h': return String(hours12);
      case 'mm': return pad(date.getMinutes());
      case 'm': return String(date.getMinutes());
      case 'ss': return pad(date.getSeconds());
      case 's': return String(date.getSeconds());
      case 'A': return date.getHours() < 12 ? 'AM' : 'PM';
      default: return date.getHours() < 12 ? 'am' : 'pm';
    }
  });
}

function ordinal(n: number): string {
  const tens = n % 100;
  if (tens >= 11 && tens <= 13) return `${n}th`;
  return `${n}${['th', 'st', 'nd', 'rd'][n % 10] ?? 'th'}`;
}
