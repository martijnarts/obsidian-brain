/**
 * Per-path in-memory buffer for the last-failed replace_window edit's
 * proposed content + search string. Used for the `edit_note({ from_buffer:
 * true })` recovery path — when an edit fails with NoMatch, the agent can
 * retry via the buffer with a lowered fuzzy threshold without having to
 * re-emit the potentially-long content payload.
 *
 * The buffer is process-global and keyed by absolute path, so vaults served
 * by one process (`obsidian-brain http`) never see each other's entries.
 */
export interface BufferEntry {
  path: string;              // absolute
  content: string;           // the replacement content
  search: string;            // the original search/needle
  mode: string;              // edit mode name
  failedAt: number;          // Date.now()
  error: string;             // failure message
}

const BUFFER_TTL_MS = 30 * 60 * 1000;   // 30 minutes
const BUFFER_SIZE = 20;                  // max entries
const MAX_ENTRY_BYTES = 512 * 1024;      // per-entry cap — refuse to buffer huge content

export class EditBuffer {
  private entries: BufferEntry[] = [];

  push(entry: BufferEntry): { buffered: boolean; reason?: string } {
    if (Buffer.byteLength(entry.content, 'utf-8') > MAX_ENTRY_BYTES) {
      return {
        buffered: false,
        reason: `content too large to buffer (>${MAX_ENTRY_BYTES} bytes); retry edit_note with fuzzy: true manually`,
      };
    }
    this.evict();
    this.entries = this.entries.filter((e) => e.path !== entry.path);
    this.entries.push(entry);
    if (this.entries.length > BUFFER_SIZE) this.entries.shift();
    return { buffered: true };
  }

  get(path: string): BufferEntry | undefined {
    this.evict();
    // Reverse loop in place of Array.prototype.findLast (ES2023) — older
    // lib.d.ts targets don't declare it, and polyfilling isn't worth the
    // dep for a one-caller method.
    for (let i = this.entries.length - 1; i >= 0; i--) {
      if (this.entries[i].path === path) return this.entries[i];
    }
    return undefined;
  }

  remove(path: string): void {
    this.entries = this.entries.filter((e) => e.path !== path);
  }

  private evict(): void {
    const now = Date.now();
    this.entries = this.entries.filter((e) => now - e.failedAt < BUFFER_TTL_MS);
  }
}

export const editBuffer = new EditBuffer();
