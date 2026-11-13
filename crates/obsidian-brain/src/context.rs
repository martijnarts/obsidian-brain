//! The runtime state of one vault that every tool handler needs.

use crate::config::VaultConfig;

/// One open vault. The index, embedder, search and pipeline join this
/// struct as their modules land.
pub struct VaultContext {
    pub config: VaultConfig,
}

impl VaultContext {
    pub async fn open(config: VaultConfig) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&config.data_dir)?;
        Ok(Self { config })
    }

    pub fn note_count(&self) -> usize {
        0
    }

    pub fn embedder_ready(&self) -> bool {
        false
    }

    pub fn reindex_in_progress(&self) -> bool {
        false
    }

    pub fn init_error(&self) -> Option<String> {
        None
    }

    pub async fn close(&self) {}
}
