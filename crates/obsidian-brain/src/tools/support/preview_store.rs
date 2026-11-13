//! The in-memory cache of `edit_note` dry-run previews.
//!
//! A preview lives 5 minutes; an expired one is evicted on the next `set`
//! or `get`, and the agent must run the dry run again. At most 50 are
//! kept; a full store evicts the oldest by `created_at`. One process serves
//! several vaults, so each preview records its vault and applies only
//! through that vault's tools.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use crate::util::js::now_ms;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingEdit {
    pub preview_id: String,
    /// The vault the preview was made in.
    pub vault_path: String,
    /// Vault-relative.
    pub path: String,
    pub original_content: String,
    pub proposed_content: String,
    /// A unified diff.
    pub diff: String,
    /// The edit mode, such as `append` or `replace_window`.
    pub mode: String,
    /// `Date.now()` when the preview was made.
    pub created_at: i64,
}

const TTL_MS: i64 = 5 * 60 * 1000;
const MAX_PREVIEWS: usize = 50;

#[derive(Debug, Default)]
pub struct PreviewStore {
    store: Mutex<HashMap<String, PendingEdit>>,
}

/// The one store of the process, shared by every vault.
pub static PREVIEW_STORE: LazyLock<PreviewStore> = LazyLock::new(PreviewStore::default);

impl PreviewStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, PendingEdit>> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set(&self, preview: PendingEdit) {
        self.set_at(preview, now_ms());
    }

    /// [`Self::set`] with the clock given, for tests.
    pub fn set_at(&self, preview: PendingEdit, now: i64) {
        let mut store = self.lock();
        evict_expired(&mut store, now);
        if store.len() >= MAX_PREVIEWS
            && let Some(oldest) = store
                .values()
                .min_by_key(|p| p.created_at)
                .map(|p| p.preview_id.clone())
        {
            store.remove(&oldest);
        }
        store.insert(preview.preview_id.clone(), preview);
    }

    pub fn get(&self, preview_id: &str) -> Option<PendingEdit> {
        self.get_at(preview_id, now_ms())
    }

    pub fn get_at(&self, preview_id: &str, now: i64) -> Option<PendingEdit> {
        let mut store = self.lock();
        evict_expired(&mut store, now);
        store.get(preview_id).cloned()
    }

    /// True when a preview was removed.
    pub fn delete(&self, preview_id: &str) -> bool {
        self.lock().remove(preview_id).is_some()
    }

    /// The number of live previews, after eviction.
    pub fn size(&self) -> usize {
        self.size_at(now_ms())
    }

    pub fn size_at(&self, now: i64) -> usize {
        let mut store = self.lock();
        evict_expired(&mut store, now);
        store.len()
    }
}

fn evict_expired(store: &mut HashMap<String, PendingEdit>, now: i64) {
    store.retain(|_, p| now - p.created_at <= TTL_MS);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview(id: &str, created_at: i64) -> PendingEdit {
        PendingEdit {
            preview_id: id.into(),
            vault_path: "/vault".into(),
            path: "note.md".into(),
            original_content: "original".into(),
            proposed_content: "proposed".into(),
            diff: "--- original\n+++ proposed\n".into(),
            mode: "append".into(),
            created_at,
        }
    }

    #[test]
    fn set_then_get_returns_the_same_preview() {
        let store = PreviewStore::default();
        let p = preview("prev_abc", now_ms());
        store.set(p.clone());
        assert_eq!(store.get("prev_abc"), Some(p));
    }

    #[test]
    fn get_on_unknown_id_returns_none() {
        assert_eq!(PreviewStore::default().get("prev_does-not-exist"), None);
    }

    #[test]
    fn get_returns_none_after_the_ttl() {
        let store = PreviewStore::default();
        let t = 1_000_000;
        store.set_at(preview("prev_ttl", t), t);
        assert!(store.get_at("prev_ttl", t).is_some());
        assert!(store.get_at("prev_ttl", t + TTL_MS + 1).is_none());
    }

    #[test]
    fn a_51st_preview_evicts_the_oldest() {
        let store = PreviewStore::default();
        let base = 1_000_000;
        for i in 0..50 {
            store.set_at(preview(&format!("prev_{i}"), base + i), base + 50);
        }
        assert_eq!(store.size_at(base + 50), 50);
        assert!(store.get_at("prev_0", base + 50).is_some());
        store.set_at(preview("prev_overflow", base + 50), base + 50);
        assert_eq!(store.size_at(base + 50), 50);
        assert!(store.get_at("prev_0", base + 50).is_none());
        assert!(store.get_at("prev_overflow", base + 50).is_some());
    }

    #[test]
    fn delete_reports_whether_it_removed_one() {
        let store = PreviewStore::default();
        store.set(preview("prev_del", now_ms()));
        assert!(store.delete("prev_del"));
        assert!(store.get("prev_del").is_none());
        assert!(!store.delete("prev_ghost"));
    }
}
