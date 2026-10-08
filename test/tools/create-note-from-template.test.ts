import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { openDb, type DatabaseHandle } from '../../src/store/db.js';
import { getNode, upsertNode } from '../../src/store/nodes.js';
import { insertEdge, getEdgesByTarget } from '../../src/store/edges.js';
import { VaultWriter } from '../../src/vault/writer.js';
import { registerCreateNoteFromTemplateTool } from '../../src/tools/create-note-from-template.js';
import { formatDate, readTemplatesConfig, renderTemplate } from '../../src/vault/templates.js';
import type { ServerContext } from '../../src/context.js';
import { makeMockServer, unwrap } from '../helpers/mock-server.js';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function unwrapError(result: any): string {
  expect(result.isError).toBe(true);
  return result.content[0].text as string;
}

// Local time, so the expected strings do not depend on the host time zone.
const FIXED = new Date(2026, 2, 5, 9, 7, 3);

describe('formatDate', () => {
  it.each([
    ['YYYY-MM-DD', '2026-03-05'],
    ['YY/M/D', '26/3/5'],
    ['HH:mm:ss', '09:07:03'],
    ['H:m:s', '9:7:3'],
    ['dddd, MMMM Do', 'Thursday, March 5th'],
    ['ddd MMM DD', 'Thu Mar 05'],
    ['dd d', 'Th 4'],
    ['hh:mm A', '09:07 AM'],
    ['h a', '9 am'],
    ['[Week of] YYYY', 'Week of 2026'],
    ['YYYY_MM', '2026_03'],
  ])('formats %s', (format, expected) => {
    expect(formatDate(FIXED, format)).toBe(expected);
  });

  it('handles afternoon, midnight and ordinal edge cases', () => {
    expect(formatDate(new Date(2026, 0, 1, 0, 0, 0), 'h A Do')).toBe('12 AM 1st');
    expect(formatDate(new Date(2026, 0, 12, 15, 0, 0), 'h:mm a Do')).toBe('3:00 pm 12th');
    expect(formatDate(new Date(2026, 0, 22), 'Do')).toBe('22nd');
    expect(formatDate(new Date(2026, 0, 23), 'Do')).toBe('23rd');
    expect(formatDate(new Date(2026, 0, 13), 'Do')).toBe('13th');
  });
});

describe('renderTemplate', () => {
  const config = { folder: 'Templates', dateFormat: 'YYYY-MM-DD', timeFormat: 'HH:mm' };

  it('fills built-ins case-insensitively and keeps unknown placeholders', () => {
    const out = renderTemplate('{{Title}} {{ date }} {{TIME}} {{date:YYYY}} {{time:HH}} {{who}} {{title:x}}', {
      title: 'T',
      now: FIXED,
      config,
      variables: { who: 'me' },
    });
    expect(out.content).toBe('T 2026-03-05 09:07 2026 09 me {{title:x}}');
    expect(out.unresolved).toEqual(['{{title:x}}']);
  });

  it('matches variables case-sensitively', () => {
    const out = renderTemplate('{{Who}}', { title: 'T', now: FIXED, config, variables: { who: 'me' } });
    expect(out.content).toBe('{{Who}}');
    expect(out.unresolved).toEqual(['{{Who}}']);
  });
});

describe('readTemplatesConfig', () => {
  let vault: string;
  beforeEach(async () => {
    vault = await mkdtemp(join(tmpdir(), 'kg-tplcfg-'));
  });
  afterEach(async () => {
    await rm(vault, { recursive: true, force: true });
  });

  it('falls back to Obsidian defaults without a settings file', () => {
    expect(readTemplatesConfig(vault)).toEqual({ folder: 'Templates', dateFormat: 'YYYY-MM-DD', timeFormat: 'HH:mm' });
  });

  it('reads folder and formats, trimming slashes and ignoring blanks', async () => {
    await mkdir(join(vault, '.obsidian'));
    await writeFile(
      join(vault, '.obsidian', 'templates.json'),
      JSON.stringify({ folder: '/Meta/Tpl/', dateFormat: 'DD.MM.YYYY', timeFormat: '' }),
    );
    expect(readTemplatesConfig(vault)).toEqual({ folder: 'Meta/Tpl', dateFormat: 'DD.MM.YYYY', timeFormat: 'HH:mm' });
  });

  it('ignores a corrupt settings file', async () => {
    await mkdir(join(vault, '.obsidian'));
    await writeFile(join(vault, '.obsidian', 'templates.json'), '{');
    expect(readTemplatesConfig(vault).folder).toBe('Templates');
  });
});

describe('create_note_from_template', () => {
  let vault: string;
  let db: DatabaseHandle;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let call: (args: any) => Promise<any>;

  beforeEach(async () => {
    vault = await mkdtemp(join(tmpdir(), 'kg-template-'));
    db = openDb(':memory:');
    await mkdir(join(vault, 'Templates'));
    await writeFile(
      join(vault, 'Templates', 'Meeting.md'),
      '---\ntype: meeting\ncreated: "{{date}} {{time}}"\n---\n# {{title}}\n\nOn {{date:dddd D MMMM}} with {{attendees}}.\n',
    );
    await writeFile(join(vault, 'Templates', 'Plain.md'), 'Plain {{title}}\n');
    const ctx = {
      db,
      writer: new VaultWriter(vault, db),
      config: { vaultPath: vault },
      ensureEmbedderReady: async () => {},
      pipeline: { index: async () => undefined },
    } as unknown as ServerContext;
    const { server, registered } = makeMockServer();
    registerCreateNoteFromTemplateTool(server, ctx);
    call = registered[0]!.cb;
  });

  afterEach(async () => {
    db.close();
    await rm(vault, { recursive: true, force: true });
  });

  it('renders the template, keeps its frontmatter, injects title and indexes the note', async () => {
    const out = unwrap(
      await call({
        template: 'Meeting',
        title: 'Kickoff',
        directory: 'Meetings',
        variables: { attendees: 'Ann' },
        date: '2026-03-05T09:07:03',
      }),
    );
    expect(out).toEqual({ path: 'Meetings/Kickoff.md', title: 'Kickoff', template: 'Templates/Meeting.md' });
    const raw = await readFile(join(vault, 'Meetings', 'Kickoff.md'), 'utf-8');
    expect(raw).toBe(
      "---\ntype: meeting\ncreated: '2026-03-05 09:07'\ntitle: Kickoff\n---\n# Kickoff\n\nOn Thursday 5 March with Ann.\n",
    );
    const node = getNode(db, 'Meetings/Kickoff.md');
    expect(node?.title).toBe('Kickoff');
    expect(node?.frontmatter.type).toBe('meeting');
  });

  it('accepts a vault-relative path with extension and a date-only date', async () => {
    const out = unwrap(await call({ template: 'Templates/Plain.md', title: 'P', date: '2026-01-02' }));
    expect(out.path).toBe('P.md');
    expect(await readFile(join(vault, 'P.md'), 'utf-8')).toBe('---\ntitle: P\n---\nPlain P\n');
  });

  it('uses the folder and formats from .obsidian/templates.json', async () => {
    await mkdir(join(vault, '.obsidian'));
    await mkdir(join(vault, 'Meta'));
    await writeFile(join(vault, '.obsidian', 'templates.json'), JSON.stringify({ folder: 'Meta', dateFormat: 'DD.MM.YYYY', timeFormat: 'h:mm A' }));
    await writeFile(join(vault, 'Meta', 'Daily.md'), '{{date}} {{time}}');
    unwrap(await call({ template: 'Daily', title: 'D', date: '2026-03-05T15:04:00' }));
    expect(await readFile(join(vault, 'D.md'), 'utf-8')).toContain('05.03.2026 3:04 PM');
  });

  it('uses the current time when no date is given', async () => {
    await writeFile(join(vault, 'Templates', 'Year.md'), '{{date:YYYY}}');
    unwrap(await call({ template: 'Year', title: 'Y' }));
    expect(await readFile(join(vault, 'Y.md'), 'utf-8')).toContain(String(new Date().getFullYear()));
  });

  it('leaves Templater syntax untouched and says so', async () => {
    await writeFile(join(vault, 'Templates', 'Tp.md'), 'Made <% tp.date.now() %> for {{title}}\n');
    const out = unwrap(await call({ template: 'Tp', title: 'T' }));
    expect(out.templater).toMatch(/does not run Templater/);
    expect(await readFile(join(vault, 'T.md'), 'utf-8')).toContain('Made <% tp.date.now() %> for T');
  });

  it('reports placeholders it could not fill', async () => {
    const out = unwrap(await call({ template: 'Meeting', title: 'M' }));
    expect(out.unresolved).toEqual(['{{attendees}}']);
  });

  it('migrates a forward-reference stub to the new note', async () => {
    upsertNode(db, { id: 'Other.md', title: 'Other', content: '[[Linked]]', frontmatter: {} });
    upsertNode(db, { id: '_stub/Linked.md', title: 'Linked', content: '', frontmatter: { _stub: true } });
    insertEdge(db, { sourceId: 'Other.md', targetId: '_stub/Linked.md', context: '[[Linked]]' });
    unwrap(await call({ template: 'Plain', title: 'Linked' }));
    expect(getNode(db, '_stub/Linked.md')).toBeUndefined();
    expect(getEdgesByTarget(db, 'Linked.md').map((e) => e.sourceId)).toEqual(['Other.md']);
  });

  it('refuses to overwrite an existing note', async () => {
    await writeFile(join(vault, 'Taken.md'), 'keep');
    expect(unwrapError(await call({ template: 'Plain', title: 'Taken' }))).toMatch(/File already exists: Taken\.md/);
    expect(await readFile(join(vault, 'Taken.md'), 'utf-8')).toBe('keep');
  });

  it('errors on a missing template', async () => {
    expect(unwrapError(await call({ template: 'Nope', title: 'X' }))).toMatch(/Template not found: "Nope"/);
    expect(existsSync(join(vault, 'X.md'))).toBe(false);
  });

  it('refuses paths that escape the vault', async () => {
    expect(unwrapError(await call({ template: '../../etc/passwd', title: 'X' }))).toMatch(/escapes the vault/);
    expect(unwrapError(await call({ template: 'Plain', title: 'X', directory: '../out' }))).toMatch(/escapes the vault/);
    expect(unwrapError(await call({ template: 'Plain', title: 'a/b' }))).toMatch(/cannot contain/);
  });

  it('refuses variables that shadow built-ins', async () => {
    expect(unwrapError(await call({ template: 'Plain', title: 'X', variables: { Title: 'y' } }))).toMatch(/cannot set Title/);
  });

  it('rejects an invalid date', async () => {
    expect(unwrapError(await call({ template: 'Plain', title: 'X', date: 'soon' }))).toMatch(/Invalid date: soon/);
  });

  it('errors clearly when substitution leaves invalid YAML', async () => {
    await writeFile(join(vault, 'Templates', 'BadFm.md'), '---\nkey: [unclosed\n---\nbody');
    expect(unwrapError(await call({ template: 'BadFm', title: 'X' }))).toMatch(/invalid YAML frontmatter/);
    expect(existsSync(join(vault, 'X.md'))).toBe(false);
  });
});
