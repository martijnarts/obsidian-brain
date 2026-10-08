import type { DatabaseHandle } from './db.js';

/**
 * Fetch the last-seen mtime for a vault-relative path, or undefined if the
 * path has never been indexed.
 */
export function getSyncMtime(db: DatabaseHandle, path: string): number | undefined {
  const row = db
    .prepare('SELECT mtime FROM sync WHERE path = ?')
    .get(path) as { mtime: number } | undefined;
  return row?.mtime;
}

/**
 * Record that `path` was indexed at mtime `mtime`. `indexedAt` defaults to
 * Date.now() if omitted — callers that run batch jobs can pin a single
 * indexed_at across many rows.
 */
export function setSyncMtime(
  db: DatabaseHandle,
  path: string,
  mtime: number,
  indexedAt: number = Date.now()
): void {
  db.prepare(
    `INSERT INTO sync (path, mtime, indexed_at) VALUES (?, ?, ?)
     ON CONFLICT(path) DO UPDATE SET
       mtime = excluded.mtime,
       indexed_at = excluded.indexed_at`
  ).run(path, mtime, indexedAt);
}

/**
 * Every path currently tracked in sync state. Used by the pipeline to detect
 * deletions (paths present in the DB but no longer on disk).
 */
export function getAllSyncPaths(db: DatabaseHandle): string[] {
  return db.prepare('SELECT path FROM sync').all().map((r) => (r as { path: string }).path);
}

/**
 * Remove the sync row for `path`. Called when a file is deleted from the vault.
 */
export function deleteSyncPath(db: DatabaseHandle, path: string): void {
  db.prepare('DELETE FROM sync WHERE path = ?').run(path);
}

/** Last-indexed mtime (ms) of every tracked path. */
export function allSyncMtimes(db: DatabaseHandle): Map<string, number> {
  const rows = db.prepare('SELECT path, mtime FROM sync').all() as Array<{ path: string; mtime: number }>;
  return new Map(rows.map((r) => [r.path, r.mtime]));
}
