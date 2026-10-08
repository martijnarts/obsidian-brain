import type { DatabaseHandle } from '../store/db.js';
import { resolveNodeName } from './name-match.js';

/**
 * Resolve a note name to exactly one indexed path. Throws when nothing
 * matches, or when the best match is a loose one (substring,
 * case-insensitive, alias) shared by several notes.
 */
export function resolveSingleNote(name: string, db: DatabaseHandle): string {
  const matches = resolveNodeName(name, db);
  if (matches.length === 0) {
    throw new Error(`No note found matching "${name}"`);
  }
  const first = matches[0]!;
  const ambiguous =
    matches.length > 1 &&
    (first.matchType === 'substring' ||
      first.matchType === 'case-insensitive' ||
      first.matchType === 'alias');
  if (ambiguous) {
    const candidates = matches
      .slice(0, 10)
      .map((m) => `- ${m.title} (${m.nodeId})`)
      .join('\n');
    throw new Error(
      `Multiple notes match "${name}". Please be more specific. Candidates:\n${candidates}`,
    );
  }
  return first.nodeId;
}
