import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { openDb, type DatabaseHandle } from '../../src/store/db.js';
import { upsertNode } from '../../src/store/nodes.js';
import { resolveSingleNote } from '../../src/resolve/single-note.js';

describe('resolveSingleNote', () => {
  let db: DatabaseHandle;

  const node = (id: string, title: string, frontmatter: Record<string, unknown> = {}): void =>
    upsertNode(db, { id, title, content: '', frontmatter });

  beforeEach(() => {
    db = openDb(':memory:');
    node('Projects/Alpha one.md', 'Alpha one', { aliases: ['A1'] });
    node('Projects/Alpha two.md', 'Alpha two', { aliases: ['a1'] });
    node('Notes/Beta.md', 'Beta');
    node('_stub/Ghost.md', 'Ghost', { _stub: true });
  });

  afterEach(() => db.close());

  it('resolves an exact path, with or without .md', () => {
    expect(resolveSingleNote('Notes/Beta.md', db)).toBe('Notes/Beta.md');
    expect(resolveSingleNote('Notes/Beta', db)).toBe('Notes/Beta.md');
  });

  it('resolves an exact title and a basename', () => {
    expect(resolveSingleNote('Beta', db)).toBe('Notes/Beta.md');
    expect(resolveSingleNote('Alpha one.md', db)).toBe('Projects/Alpha one.md');
  });

  it('resolves a fuzzy match that has one candidate', () => {
    expect(resolveSingleNote('beta', db)).toBe('Notes/Beta.md');
    expect(resolveSingleNote('pha tw', db)).toBe('Projects/Alpha two.md');
  });

  it('lists the candidates when a loose match is ambiguous', () => {
    expect(() => resolveSingleNote('alpha', db)).toThrow(
      'Multiple notes match "alpha". Please be more specific. Candidates:\n' +
        '- Alpha one (Projects/Alpha one.md)\n- Alpha two (Projects/Alpha two.md)',
    );
    expect(() => resolveSingleNote('A1', db)).toThrow(/Multiple notes match "A1"/);
  });

  it('caps the candidate list at ten', () => {
    for (let i = 0; i < 12; i++) node(`Many/Gamma ${i}.md`, `Gamma ${i}`);
    let message = '';
    try {
      resolveSingleNote('gamma', db);
    } catch (err) {
      message = (err as Error).message;
    }
    expect(message.split('\n').filter((l) => l.startsWith('- '))).toHaveLength(10);
  });

  it('errors when nothing matches', () => {
    expect(() => resolveSingleNote('nothing here', db)).toThrow('No note found matching "nothing here"');
  });

  it('refuses a stub unless allowStubs is set', () => {
    expect(() => resolveSingleNote('_stub/Ghost.md', db)).toThrow(
      '"_stub/Ghost.md" is an unresolved link target, not a note on disk.',
    );
    expect(() => resolveSingleNote('Ghost', db)).toThrow(/unresolved link target/);
    expect(resolveSingleNote('Ghost', db, { allowStubs: true })).toBe('_stub/Ghost.md');
  });

  it('drops stubs before the ambiguity check', () => {
    node('_stub/Delta.md', 'Delta', { _stub: true });
    node('Notes/Delta.md', 'Delta');
    expect(resolveSingleNote('delta', db)).toBe('Notes/Delta.md');
    expect(() => resolveSingleNote('delta', db, { allowStubs: true })).toThrow(/Multiple notes match/);
  });
});
