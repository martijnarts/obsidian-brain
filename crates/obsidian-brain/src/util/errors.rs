//! Error messages in the form clients see them.

use std::fmt::Display;
use std::io;
use std::path::Path;

/// The message of an error, for a `catch` that reports it.
pub fn error_message(err: &dyn Display) -> String {
    err.to_string()
}

/// The Node.js code and description of an I/O error, such as
/// `("ENOENT", "no such file or directory")`.
fn node_code(err: &io::Error) -> Option<(&'static str, &'static str)> {
    use io::ErrorKind as K;
    // EPERM is 1 on Linux and macOS; Rust folds it into PermissionDenied.
    if err.raw_os_error() == Some(1) {
        return Some(("EPERM", "operation not permitted"));
    }
    Some(match err.kind() {
        K::NotFound => ("ENOENT", "no such file or directory"),
        K::PermissionDenied => ("EACCES", "permission denied"),
        K::AlreadyExists => ("EEXIST", "file already exists"),
        K::NotADirectory => ("ENOTDIR", "not a directory"),
        K::IsADirectory => ("EISDIR", "illegal operation on a directory"),
        K::DirectoryNotEmpty => ("ENOTEMPTY", "directory not empty"),
        K::ReadOnlyFilesystem => ("EROFS", "read-only file system"),
        K::StorageFull => ("ENOSPC", "no space left on device"),
        K::CrossesDevices => ("EXDEV", "cross-device link not permitted"),
        K::ResourceBusy => ("EBUSY", "resource busy or locked"),
        K::InvalidFilename => ("ENAMETOOLONG", "name too long"),
        K::InvalidInput => ("EINVAL", "invalid argument"),
        _ => return None,
    })
}

/// The message Node.js gives a failed `fs` call, such as
/// `ENOENT: no such file or directory, open '/v/a.md'`. `dest` is the
/// second path of a `rename` or `copyfile`.
pub fn node_fs_error_message(
    err: &io::Error,
    syscall: &str,
    path: &Path,
    dest: Option<&Path>,
) -> String {
    let Some((code, desc)) = node_code(err) else {
        return err.to_string();
    };
    match dest {
        Some(d) => format!(
            "{code}: {desc}, {syscall} '{}' -> '{}'",
            path.display(),
            d.display()
        ),
        None => format!("{code}: {desc}, {syscall} '{}'", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_node() {
        let err = std::fs::read("/definitely/not/here").unwrap_err();
        assert_eq!(
            node_fs_error_message(&err, "open", Path::new("/definitely/not/here"), None),
            "ENOENT: no such file or directory, open '/definitely/not/here'"
        );
        assert_eq!(
            node_fs_error_message(&err, "rename", Path::new("/a"), Some(Path::new("/b"))),
            "ENOENT: no such file or directory, rename '/a' -> '/b'"
        );
    }
}
