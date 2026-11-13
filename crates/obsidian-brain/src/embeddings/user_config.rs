//! The per-user config dir, outside the install, that holds
//! `model-overrides.json`.
//!
//! Path resolution:
//!   - `$OBSIDIAN_BRAIN_CONFIG_DIR` when set
//!   - Windows: `%APPDATA%/obsidian-brain/`
//!   - else `$XDG_CONFIG_HOME/obsidian-brain/`, or `~/.config/obsidian-brain/`
//!
//! The dir may not exist yet; nothing here creates it.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::util::js;

pub fn get_user_config_dir_from(env: &HashMap<String, String>) -> PathBuf {
    if let Some(explicit) = js::env_value(env, "OBSIDIAN_BRAIN_CONFIG_DIR") {
        return PathBuf::from(explicit);
    }
    if cfg!(windows)
        && let Some(appdata) = env.get("APPDATA").filter(|v| !v.is_empty())
    {
        return PathBuf::from(appdata).join("obsidian-brain");
    }
    if let Some(xdg) = js::env_value(env, "XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("obsidian-brain");
    }
    let home = env.get("HOME").map(String::as_str).unwrap_or("/");
    PathBuf::from(home).join(".config").join("obsidian-brain")
}

pub fn get_user_config_dir() -> PathBuf {
    get_user_config_dir_from(&js::process_env())
}

/// Path to the user model-overrides JSON.
pub fn get_overrides_path() -> PathBuf {
    get_user_config_dir().join("model-overrides.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn explicit_dir_wins() {
        let e = env(&[
            ("OBSIDIAN_BRAIN_CONFIG_DIR", " /tmp/x "),
            ("XDG_CONFIG_HOME", "/xdg"),
        ]);
        assert_eq!(get_user_config_dir_from(&e), PathBuf::from("/tmp/x"));
    }

    #[test]
    fn xdg_then_home() {
        let e = env(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/u")]);
        assert_eq!(
            get_user_config_dir_from(&e),
            PathBuf::from("/xdg/obsidian-brain")
        );
        let e = env(&[("HOME", "/home/u")]);
        assert_eq!(
            get_user_config_dir_from(&e),
            PathBuf::from("/home/u/.config/obsidian-brain")
        );
    }
}
