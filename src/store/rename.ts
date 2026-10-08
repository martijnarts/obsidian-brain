import type { DatabaseHandle } from './db.js';

/**
 * Atomically rewrite every row keyed on `oldId` to use `newId` instead. This
 * is the primitive behind `move_note`: by updating node / edge / chunk / sync
 * rows in place, inbound edges survive the rename without the
 * delete-then-upsert dance that the indexer's deletion-detection loop would
 * otherwise do (which drops every inbound edge).
 *
 * Tables touched, in transaction order:
 *
 *   1. `chunks`  — composite PK `${nodeId}::${chunkIndex}`. Rewrite both the
 *                  id and node_id in one statement.
 *   2. `edges`   — both `source_id` and `target_id`; no FK enforcement.
 *   3. `sync`    — the path column shares the node-id namespace.
 *   4. `nodes`   — the text PK. SQLite rowid is separate and unchanged, so
 *                  `nodes_fts` (content='nodes', content_rowid='rowid') and
 *                  `nodes_vec` (keyed by rowid) stay valid with no action.
 *
 * `chunks_vec` is rowid-keyed; `chunks.rowid` doesn't change when we UPDATE
 * the composite id, so no vec-table action is needed.
 *
 * Stub absorption (e.g. `_stub/<oldStem>.md` → `newId`) is deliberately NOT
 * handled here — that is vault-semantic coupling. Callers should run
 * `migrateStubToReal` from `nodes.ts` as a separate step if they want the
 * renamed target to absorb any surviving forward-reference stub.
 */
export function renameNode(db: DatabaseHandle, oldId: string, newId: string): void {
  if (oldId === newId) return;

  const tx = db.transaction(() => {
    // better-sqlite3 enables `PRAGMA foreign_keys = ON` by default (unlike
    // standard sqlite3), and `chunks.node_id REFERENCES nodes(id)` would
    // otherwise fire mid-transaction because neither {nodes.id := newId,
    // chunks.node_id := oldId} nor {nodes.id := oldId, chunks.node_id :=
    // newId} is transiently valid. Defer the check to commit time so we can
    // update both sides in the transaction.
    db.pragma('defer_foreign_keys = ON');

    // 1. chunks: composite PK `${nodeId}::${chunkIndex}` + FK node_id.
    //    Rewrite both columns in the same statement so there's no moment at
    //    which the id/node_id pair would violate the composite-id contract.
    db.prepare(
      `UPDATE chunks SET id = (? || '::' || chunk_index), node_id = ?
       WHERE node_id = ?`,
    ).run(newId, newId, oldId);

    // 2. edges (both directions).
    db.prepare('UPDATE edges SET source_id = ? WHERE source_id = ?').run(newId, oldId);
    db.prepare('UPDATE edges SET target_id = ? WHERE target_id = ?').run(newId, oldId);

    // 3. sync.path shares the node-id namespace.
    db.prepare('UPDATE sync SET path = ? WHERE path = ?').run(newId, oldId);

    // 4. nodes.id last. rowid unchanged → nodes_fts + nodes_vec stay valid.
    db.prepare('UPDATE nodes SET id = ? WHERE id = ?').run(newId, oldId);
  });

  tx();
}
