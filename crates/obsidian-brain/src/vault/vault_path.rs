//! Turn a client-supplied path into a vault-relative path and an absolute
//! path, refusing anything that would land outside the vault.
//!
//! Four escapes are refused: a NUL byte, an absolute path, a `..` segment
//! that climbs above the root, and a symlink whose target lies outside the
//! vault. The symlink check resolves the deepest part of the path that
//! exists, so it also covers a path that does not exist yet (a folder or
//! file to create).

use std::io;
use std::path::{Component, Path, PathBuf};

use anyhow::{Result, anyhow, bail};

use crate::util::errors::node_fs_error_message;
use crate::util::js;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultPath {
    /// Vault-relative, `/`-separated, no leading or trailing slash. `""` is the root.
    pub rel: String,
    pub abs: PathBuf,
}

/// `path.resolve(base, rel)`: absolute and lexically normalised, without
/// touching the disk.
fn resolve(base: &Path, rel: &str) -> PathBuf {
    let mut joined = if base.is_absolute() {
        base.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(base)
    };
    if !rel.is_empty() {
        joined.push(rel);
    }
    normalize_abs(&joined)
}

fn normalize_abs(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

pub fn resolve_vault_path(vault_path: &Path, input: &str) -> Result<VaultPath> {
    let rel = lexical_rel(input)?;
    let abs = resolve(vault_path, &rel);
    let root = std::fs::canonicalize(vault_path)
        .map_err(|e| anyhow!(node_fs_error_message(&e, "realpath", vault_path, None)))?;
    let mut existing = abs.clone();
    while std::fs::symlink_metadata(&existing).is_err() {
        match existing.parent() {
            Some(parent) => existing = parent.to_path_buf(),
            None => break,
        }
    }
    // A dangling symlink exists but has no real path; a write through it would
    // create its target, which may lie anywhere.
    let outside = || anyhow!("Path \"{input}\" is outside the vault through a symlink");
    let real = std::fs::canonicalize(&existing).map_err(|_| outside())?;
    if real != root && !real.starts_with(&root) {
        return Err(outside());
    }
    Ok(VaultPath { rel, abs })
}

/// `path.posix.normalize` for a relative path, without the trailing slash.
fn posix_normalize_rel(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}

/// The checks that need no disk: NUL, absolute paths and `..` escapes.
fn lexical_rel(input: &str) -> Result<String> {
    if input.contains('\0') {
        bail!(
            "Path {} contains a NUL byte",
            serde_json::to_string(input).expect("strings serialize")
        );
    }
    let cleaned = js::trim(input).replace('\\', "/");
    let drive = cleaned
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
        && cleaned.as_bytes().get(1) == Some(&b':');
    if cleaned.starts_with('/') || drive {
        bail!("Path \"{input}\" is outside the vault: use a vault-relative path");
    }
    let rel = posix_normalize_rel(if cleaned.is_empty() { "." } else { &cleaned });
    if rel == ".." || rel.starts_with("../") {
        bail!("Path \"{input}\" is outside the vault");
    }
    Ok(if rel == "." { String::new() } else { rel })
}

/// Vault-relative form of `abs`, always with `/` separators.
pub fn to_vault_relative(vault_path: &Path, abs: &Path) -> String {
    let base = resolve(vault_path, "");
    let target = normalize_abs(abs);
    let base_parts: Vec<_> = base.components().collect();
    let target_parts: Vec<_> = target.components().collect();
    let common = base_parts
        .iter()
        .zip(&target_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut out: Vec<String> = vec!["..".into(); base_parts.len() - common];
    out.extend(
        target_parts[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    out.join("/")
}

/// Normalise a client-supplied folder scope to `a/b` form, with the same
/// lexical checks as `resolve_vault_path`. A folder scope is always rooted
/// at the vault, so leading slashes are dropped: `/a/` is `a`, and an
/// absent folder, `""` or `/` is the root (`""`).
pub fn normalize_folder(folder: Option<&str>) -> Result<String> {
    let Some(folder) = folder else {
        return Ok(String::new());
    };
    let cleaned = js::trim(folder).replace('\\', "/");
    lexical_rel(cleaned.trim_start_matches('/'))
}

/// True when the vault-relative `id` lies under `folder` (`""` is the root).
pub fn in_folder(id: &str, folder: &str) -> bool {
    folder.is_empty() || id.starts_with(&format!("{folder}/"))
}

/// Resolve a folder scope on disk: normalise it, guard it, and check that
/// it is a directory. A symlinked folder passes when its target is inside
/// the vault.
pub fn resolve_folder(vault_path: &Path, folder: Option<&str>) -> Result<VaultPath> {
    let resolved = resolve_vault_path(vault_path, &normalize_folder(folder)?)?;
    if !resolved.abs.is_dir() {
        bail!("Folder not found: \"{}\"", folder.unwrap_or(""));
    }
    Ok(resolved)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteFile {
    pub rel: String,
    pub abs: PathBuf,
    pub content: String,
}

/// Guard a vault-relative path and read the file as UTF-8.
pub fn read_note_file(vault_path: &Path, input: &str) -> Result<NoteFile> {
    let VaultPath { rel, abs } = resolve_vault_path(vault_path, input)?;
    match read_utf8(&abs) {
        Ok(content) => Ok(NoteFile { rel, abs, content }),
        Err(e) => bail!(
            "Could not read \"{rel}\": {}",
            node_fs_error_message(&e, "open", &abs, None)
        ),
    }
}

/// Read a file as UTF-8, replacing invalid sequences with U+FFFD as Node's
/// `readFile(path, 'utf-8')` does.
pub fn read_utf8(abs: &Path) -> io::Result<String> {
    let bytes = std::fs::read(abs)?;
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}

/// Atomic replace: write a uniquely named temp file next to the target, then
/// rename it over the target, so a reader never sees half a file and two
/// writers never share a temp file.
pub fn write_file_atomic(abs: &Path, content: impl AsRef<[u8]>) -> io::Result<()> {
    let mut suffix = [0u8; 6];
    getrandom::fill(&mut suffix).map_err(io::Error::other)?;
    let mut tmp = abs.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", hex::encode(suffix)));
    let tmp = PathBuf::from(tmp);
    let result = std::fs::write(&tmp, content).and_then(|()| std::fs::rename(&tmp, abs));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct Dirs {
        vault: tempfile::TempDir,
        outside: tempfile::TempDir,
    }

    fn setup() -> Dirs {
        let vault = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(vault.path().join("A")).unwrap();
        std::fs::write(vault.path().join("A").join("Note.md"), "x").unwrap();
        std::fs::write(outside.path().join("Secret.md"), "secret").unwrap();
        Dirs { vault, outside }
    }

    fn err(r: Result<impl std::fmt::Debug>) -> String {
        r.unwrap_err().to_string()
    }

    #[test]
    fn accepts_paths_inside_the_vault_existing_or_not() {
        let d = setup();
        let v = d.vault.path();
        assert_eq!(
            resolve_vault_path(v, "A/Note.md").unwrap(),
            VaultPath {
                rel: "A/Note.md".into(),
                abs: v.join("A/Note.md")
            }
        );
        assert_eq!(
            resolve_vault_path(v, "A/../A/New.md").unwrap(),
            VaultPath {
                rel: "A/New.md".into(),
                abs: v.join("A/New.md")
            }
        );
        assert_eq!(
            resolve_vault_path(v, "New/Deep/x.md").unwrap().rel,
            "New/Deep/x.md"
        );
        assert_eq!(
            resolve_vault_path(v, "").unwrap(),
            VaultPath {
                rel: String::new(),
                abs: v.to_path_buf()
            }
        );
        assert_eq!(
            resolve_vault_path(v, ".").unwrap(),
            VaultPath {
                rel: String::new(),
                abs: v.to_path_buf()
            }
        );
    }

    #[test]
    fn uses_slash_separators_and_drops_trailing_slashes() {
        let d = setup();
        assert_eq!(
            resolve_vault_path(d.vault.path(), "A\\Note.md")
                .unwrap()
                .rel,
            "A/Note.md"
        );
        assert_eq!(resolve_vault_path(d.vault.path(), "A/").unwrap().rel, "A");
    }

    #[test]
    fn refuses_absolute_paths() {
        let d = setup();
        let inside = d.vault.path().join("A/Note.md");
        for p in [
            inside.to_str().unwrap(),
            "/etc/passwd",
            "\\etc",
            "C:/temp",
            "c:\\temp",
        ] {
            assert!(
                err(resolve_vault_path(d.vault.path(), p))
                    .contains("outside the vault: use a vault-relative path"),
                "{p}"
            );
        }
    }

    #[test]
    fn refuses_dotdot_escapes() {
        let d = setup();
        for p in ["..", "../x.md", "A/../../x.md", "..\\x.md"] {
            assert_eq!(
                err(resolve_vault_path(d.vault.path(), p)),
                format!("Path \"{p}\" is outside the vault")
            );
        }
    }

    #[test]
    fn refuses_nul_bytes() {
        let d = setup();
        assert_eq!(
            err(resolve_vault_path(d.vault.path(), "A/x\0.md")),
            "Path \"A/x\\u0000.md\" contains a NUL byte"
        );
    }

    #[test]
    fn refuses_symlinks_that_point_outside() {
        let d = setup();
        let v = d.vault.path();
        symlink(d.outside.path().join("Secret.md"), v.join("Linked.md")).unwrap();
        symlink(d.outside.path(), v.join("Out")).unwrap();
        for p in ["Linked.md", "Out", "Out/Secret.md", "Out/new/deeper/x.md"] {
            assert!(
                err(resolve_vault_path(v, p)).contains("outside the vault through a symlink"),
                "{p}"
            );
        }
    }

    #[test]
    fn refuses_a_dangling_symlink_and_paths_under_it() {
        let d = setup();
        symlink(
            d.outside.path().join("missing"),
            d.vault.path().join("dangling"),
        )
        .unwrap();
        assert!(err(resolve_vault_path(d.vault.path(), "dangling")).contains("through a symlink"));
        assert!(
            err(resolve_vault_path(d.vault.path(), "dangling/x.md")).contains("through a symlink")
        );
    }

    #[test]
    fn accepts_a_symlink_whose_target_stays_inside_the_vault() {
        let d = setup();
        let v = d.vault.path();
        symlink(v.join("A"), v.join("Alias")).unwrap();
        assert_eq!(
            resolve_vault_path(v, "Alias/Note.md").unwrap().abs,
            v.join("Alias/Note.md")
        );
        assert_eq!(
            resolve_vault_path(v, "Alias/New.md").unwrap().rel,
            "Alias/New.md"
        );
    }

    #[test]
    fn to_vault_relative_uses_slashes() {
        let d = setup();
        let v = d.vault.path();
        assert_eq!(
            to_vault_relative(v, &v.join("A").join("Note.md")),
            "A/Note.md"
        );
        assert_eq!(to_vault_relative(v, v), "");
    }

    #[test]
    fn normalize_folder_and_in_folder() {
        assert_eq!(normalize_folder(Some("/a\\b/")).unwrap(), "a/b");
        assert_eq!(normalize_folder(Some("./a/./b")).unwrap(), "a/b");
        assert_eq!(normalize_folder(Some(" a/b ")).unwrap(), "a/b");
        for f in [None, Some(""), Some("/"), Some(".")] {
            assert_eq!(normalize_folder(f).unwrap(), "");
        }
        assert!(err(normalize_folder(Some(".."))).contains("outside the vault"));
        assert!(err(normalize_folder(Some("a/../../b"))).contains("outside the vault"));
        assert!(err(normalize_folder(Some("C:/x"))).contains("vault-relative"));
        assert!(err(normalize_folder(Some("a\0"))).contains("NUL byte"));
        assert!(in_folder("a/b/c.md", ""));
        assert!(in_folder("a/b/c.md", "a"));
        assert!(in_folder("a/b/c.md", "a/b"));
        assert!(!in_folder("ab/c.md", "a"));
        assert!(!in_folder("a.md", "a"));
    }

    #[test]
    fn resolve_folder_checks_for_a_directory() {
        let d = setup();
        let v = d.vault.path();
        assert_eq!(
            resolve_folder(v, Some("/A/")).unwrap(),
            VaultPath {
                rel: "A".into(),
                abs: v.join("A")
            }
        );
        assert_eq!(
            resolve_folder(v, None).unwrap(),
            VaultPath {
                rel: String::new(),
                abs: v.to_path_buf()
            }
        );
        assert_eq!(
            err(resolve_folder(v, Some("Nope"))),
            "Folder not found: \"Nope\""
        );
        assert_eq!(
            err(resolve_folder(v, Some("A/Note.md"))),
            "Folder not found: \"A/Note.md\""
        );
        symlink(d.outside.path(), v.join("Out")).unwrap();
        assert!(err(resolve_folder(v, Some("Out"))).contains("through a symlink"));
        assert!(err(resolve_folder(v, Some("../x"))).contains("outside the vault"));
    }

    #[test]
    fn read_note_file_reads_and_guards() {
        let d = setup();
        let v = d.vault.path();
        assert_eq!(
            read_note_file(v, "A/Note.md").unwrap(),
            NoteFile {
                rel: "A/Note.md".into(),
                abs: v.join("A/Note.md"),
                content: "x".into()
            }
        );
        assert!(
            err(read_note_file(v, "A/Gone.md")).starts_with("Could not read \"A/Gone.md\": ENOENT")
        );
        symlink(d.outside.path().join("Secret.md"), v.join("Linked.md")).unwrap();
        assert!(err(read_note_file(v, "Linked.md")).contains("outside the vault"));
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn write_file_atomic_replaces_and_cleans_up() {
        let d = setup();
        let a = d.vault.path().join("A");
        write_file_atomic(&a.join("Note.md"), "new ü").unwrap();
        assert_eq!(std::fs::read_to_string(a.join("Note.md")).unwrap(), "new ü");
        assert_eq!(listing(&a), ["Note.md"]);
        let bytes = [0u8, 255, 1, 128, 10];
        write_file_atomic(&a.join("blob.bin"), bytes).unwrap();
        assert_eq!(std::fs::read(a.join("blob.bin")).unwrap(), bytes);
        assert_eq!(listing(&a), ["Note.md", "blob.bin"]);
    }

    #[test]
    fn write_file_atomic_lets_concurrent_writers_finish() {
        let d = setup();
        let target = d.vault.path().join("A").join("Note.md");
        std::thread::scope(|s| {
            for c in ["one", "two", "three"] {
                let t = &target;
                s.spawn(move || write_file_atomic(t, c).unwrap());
            }
        });
        assert!(
            ["one", "two", "three"].contains(&std::fs::read_to_string(&target).unwrap().as_str())
        );
        assert_eq!(listing(&d.vault.path().join("A")), ["Note.md"]);
    }

    #[test]
    fn write_file_atomic_removes_the_temp_file_when_the_rename_fails() {
        let d = setup();
        let a = d.vault.path().join("A");
        std::fs::create_dir(a.join("Dir")).unwrap();
        std::fs::write(a.join("Dir").join("child"), "").unwrap();
        assert!(write_file_atomic(&a.join("Dir"), "x").is_err());
        assert_eq!(listing(&a), ["Dir", "Note.md"]);
    }
}
