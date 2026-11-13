use std::env;
use std::path::{Path, PathBuf};

/// The data dir: the override, then `DATA_DIR`, then `KG_DATA_DIR`, then
/// `$XDG_DATA_HOME/obsidian-brain`. Each vault keeps its index in
/// `<data dir>/<name>`.
pub fn resolve_data_dir(over: Option<&Path>) -> PathBuf {
    if let Some(dir) = over {
        return dir.to_path_buf();
    }
    for key in ["DATA_DIR", "KG_DATA_DIR"] {
        if let Some(dir) = env::var_os(key).filter(|v| !v.is_empty()) {
            return PathBuf::from(dir);
        }
    }
    let xdg = env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".local").join("share"));
    xdg.join("obsidian-brain")
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// The config of one vault.
#[derive(Debug, Clone)]
pub struct VaultConfig {
    /// The value of the `vault` tool argument.
    pub name: String,
    pub vault_path: PathBuf,
    /// `<data dir>/<name>`.
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
}

impl VaultConfig {
    pub fn new(name: &str, vault_path: &Path, data_dir: &Path) -> Self {
        let data_dir = data_dir.join(name);
        Self {
            name: name.to_string(),
            vault_path: vault_path.to_path_buf(),
            // A new file name, so the TypeScript index is never read by mistake.
            db_path: data_dir.join("index.db"),
            data_dir,
        }
    }
}
