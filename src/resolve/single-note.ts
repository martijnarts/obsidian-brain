import type { DatabaseHandle } from '../store/db.js';
import { resolveNodeName } from './name-match.js';

/**
 * Resolve a note name to exactly one indexed path. Throws when nothing
 * matches, or when the best match is a loose one (substring,
 * case-insensitive, alias) shared by several notes.
 *
 * Unresolved link targets (`_stub/` ids) are refused unless `allowStubs`
 * is set, because they have no file on disk. Index-only tools (graph
 * queries, index reads, delete) pass `allowStubs: true`.
 */
export function resolveSingleNote(
  name: string,
  db: DatabaseHandle,
  opts: { allowStubs?: boolean } = {},
): string {
  let matches = resolveNodeName(name, db);
  if (opts.allowStubs !== true) {
    const real = matches.filter((m) => !m.nodeId.startsWith('_stub/'));
    if (real.length === 0 && matches.length > 0) {
      throw new Error(`"${matches[0]!.nodeId}" is an unresolved link target, not a note on disk.`);
    }
    matches = real;
  }
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
