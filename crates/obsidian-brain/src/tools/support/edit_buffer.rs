//! The last failed `replace_window` edit of each note: its replacement
//! content and search string. `edit_note({ from_buffer: true })` retries it
//! with a lower fuzzy threshold, without the agent sending the content
//! again.
//!
//! The buffer is process-wide and keyed by absolute path, so the vaults of
//! one process never see each other's entries. Entries live 30 minutes; at
//! most 20 are kept.

use std::sync::{LazyLock, Mutex};

use crate::util::js::now_ms;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferEntry {
    /// Absolute.
    pub path: String,
    /// The replacement content.
    pub content: String,
    /// The original search string.
    pub search: String,
    /// The edit mode.
    pub mode: String,
    /// `Date.now()` of the failure.
    pub failed_at: i64,
    /// The failure message.
    pub error: String,
}

/// Whether [`EditBuffer::push`] kept the entry, and why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushOutcome {
    pub buffered: bool,
    pub reason: Option<String>,
}

const BUFFER_TTL_MS: i64 = 30 * 60 * 1000;
const BUFFER_SIZE: usize = 20;
/// Content larger than this is not buffered.
const MAX_ENTRY_BYTES: usize = 512 * 1024;

#[derive(Debug, Default)]
pub struct EditBuffer {
    entries: Mutex<Vec<BufferEntry>>,
}

/// The one buffer of the process.
pub static EDIT_BUFFER: LazyLock<EditBuffer> = LazyLock::new(EditBuffer::default);

impl EditBuffer {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<BufferEntry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn push(&self, entry: BufferEntry) -> PushOutcome {
        self.push_at(entry, now_ms())
    }

    /// [`Self::push`] with the clock given, for tests.
    pub fn push_at(&self, entry: BufferEntry, now: i64) -> PushOutcome {
        if entry.content.len() > MAX_ENTRY_BYTES {
            return PushOutcome {
                buffered: false,
                reason: Some(format!(
                    "content too large to buffer (>{MAX_ENTRY_BYTES} bytes); retry edit_note with fuzzy: true manually"
                )),
            };
        }
        let mut entries = self.lock();
        evict(&mut entries, now);
        entries.retain(|e| e.path != entry.path);
        entries.push(entry);
        if entries.len() > BUFFER_SIZE {
            entries.remove(0);
        }
        PushOutcome {
            buffered: true,
            reason: None,
        }
    }

    pub fn get(&self, path: &str) -> Option<BufferEntry> {
        self.get_at(path, now_ms())
    }

    pub fn get_at(&self, path: &str, now: i64) -> Option<BufferEntry> {
        let mut entries = self.lock();
        evict(&mut entries, now);
        entries.iter().rev().find(|e| e.path == path).cloned()
    }

    pub fn remove(&self, path: &str) {
        self.lock().retain(|e| e.path != path);
    }
}

fn evict(entries: &mut Vec<BufferEntry>, now: i64) {
    entries.retain(|e| now - e.failed_at < BUFFER_TTL_MS);
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_000_000;

    fn entry(path: &str, content: &str) -> BufferEntry {
        BufferEntry {
            path: path.into(),
            content: content.into(),
            search: "original text".into(),
            mode: "replace_window".into(),
            failed_at: T,
            error: "[replace_window] NoMatch: search text not found".into(),
        }
    }

    #[test]
    fn push_then_get_returns_the_same_entry() {
        let buf = EditBuffer::default();
        let e = entry("notes/foo.md", "replacement content");
        buf.push_at(e.clone(), T);
        assert_eq!(buf.get_at("notes/foo.md", T), Some(e));
        assert_eq!(buf.get_at("notes/nonexistent.md", T), None);
    }

    #[test]
    fn a_second_push_for_the_same_path_replaces() {
        let buf = EditBuffer::default();
        buf.push_at(entry("notes/foo.md", "first content"), T);
        buf.push_at(entry("notes/foo.md", "second content"), T);
        buf.push_at(entry("notes/bar.md", "bar content"), T);
        assert_eq!(
            buf.get_at("notes/foo.md", T).unwrap().content,
            "second content"
        );
    }

    #[test]
    fn get_returns_none_after_the_ttl() {
        let buf = EditBuffer::default();
        buf.push_at(entry("notes/foo.md", "x"), T);
        assert!(buf.get_at("notes/foo.md", T).is_some());
        assert!(buf.get_at("notes/foo.md", T + BUFFER_TTL_MS + 1).is_none());
    }

    #[test]
    fn a_21st_path_evicts_the_oldest() {
        let buf = EditBuffer::default();
        for i in 0..20 {
            buf.push_at(entry(&format!("notes/file-{i}.md"), "x"), T);
        }
        assert!(buf.get_at("notes/file-0.md", T).is_some());
        buf.push_at(entry("notes/file-20.md", "x"), T);
        assert!(buf.get_at("notes/file-0.md", T).is_none());
        assert!(buf.get_at("notes/file-20.md", T).is_some());
        assert!(buf.get_at("notes/file-1.md", T).is_some());
        assert!(buf.get_at("notes/file-19.md", T).is_some());
    }

    #[test]
    fn content_over_512_kib_is_refused_and_at_the_limit_kept() {
        let buf = EditBuffer::default();
        let out = buf.push_at(entry("notes/foo.md", &"x".repeat(512 * 1024 + 1)), T);
        assert!(!out.buffered);
        assert!(out.reason.unwrap().contains("content too large to buffer"));
        assert!(buf.get_at("notes/foo.md", T).is_none());
        let out = buf.push_at(entry("notes/foo.md", &"x".repeat(512 * 1024)), T);
        assert!(out.buffered);
        assert!(buf.get_at("notes/foo.md", T).is_some());
    }

    #[test]
    fn remove_clears_a_path_and_ignores_unknown_ones() {
        let buf = EditBuffer::default();
        buf.push_at(entry("notes/foo.md", "x"), T);
        buf.remove("notes/nonexistent.md");
        assert!(buf.get_at("notes/foo.md", T).is_some());
        buf.remove("notes/foo.md");
        assert!(buf.get_at("notes/foo.md", T).is_none());
    }
}
