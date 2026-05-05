//! Path validation rules from the spec.
//!
//! All installable paths are *relative* to the sandbox root. The installer
//! must reject:
//!
//! - absolute paths
//! - `.` and `..` traversal
//! - entries that normalize outside the sandbox root
//! - hardlink targets that are absolute or escape
//! - symlink *link* paths (the entry name) that escape
//!
//! Symlink *targets* can be relative paths that point anywhere on the
//! installed sandbox filesystem; that's a runtime concern. We only validate
//! the entry name itself here.

use crate::error::{Error, Result};

/// Normalize a relative POSIX-style path. Returns `None` if the path
/// references the parent directory in a way that escapes the root.
pub fn normalize(input: &str) -> Option<String> {
    if input.starts_with('/') {
        return None;
    }
    let mut out: Vec<&str> = Vec::new();
    for part in input.split('/') {
        match part {
            "" | "." => continue,
            ".." => {
                out.pop()?;
            }
            seg => out.push(seg),
        }
    }
    Some(out.join("/"))
}

/// Validate a path that will become an archive entry name. Rejects
/// absolute paths, empty strings, dot-only paths, and traversal escapes.
pub fn validate_entry_path(path: &str) -> Result<String> {
    if path.is_empty() {
        return Err(Error::InvalidPath {
            path: path.to_string(),
            reason: "empty",
        });
    }
    if path.starts_with('/') {
        return Err(Error::InvalidPath {
            path: path.to_string(),
            reason: "absolute path is not allowed",
        });
    }
    let normalized = normalize(path).ok_or_else(|| Error::InvalidPath {
        path: path.to_string(),
        reason: "path traversal escapes archive root",
    })?;
    if normalized.is_empty() {
        return Err(Error::InvalidPath {
            path: path.to_string(),
            reason: "path normalizes to empty",
        });
    }
    Ok(normalized)
}

/// Validate a hardlink target stored in a tar entry.
///
/// Same shape as an entry path: relative, no traversal escape, no
/// absolute paths. Symlink targets are *not* validated here — they can
/// legitimately be relative paths with `..` segments inside the sandbox.
pub fn validate_hardlink_target(path: &str) -> Result<String> {
    if path.starts_with('/') {
        return Err(Error::InvalidPath {
            path: path.to_string(),
            reason: "hardlink target is absolute",
        });
    }
    normalize(path).ok_or_else(|| Error::InvalidPath {
        path: path.to_string(),
        reason: "hardlink target escapes archive root",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_dot_segments() {
        assert_eq!(normalize("a/./b/./c"), Some("a/b/c".to_string()));
    }

    #[test]
    fn normalize_resolves_parent_within_bounds() {
        assert_eq!(normalize("a/b/../c"), Some("a/c".to_string()));
    }

    #[test]
    fn normalize_rejects_root_escape() {
        assert_eq!(normalize("../etc/passwd"), None);
        assert_eq!(normalize("a/../../etc"), None);
    }

    #[test]
    fn normalize_rejects_absolute() {
        assert_eq!(normalize("/etc/passwd"), None);
    }

    #[test]
    fn validate_entry_path_strips_dots() {
        assert_eq!(validate_entry_path("./a/b").unwrap(), "a/b");
    }

    #[test]
    fn validate_entry_path_rejects_traversal() {
        assert!(validate_entry_path("../etc/passwd").is_err());
        assert!(validate_entry_path("a/../../b").is_err());
    }

    #[test]
    fn validate_entry_path_rejects_absolute() {
        assert!(validate_entry_path("/etc/passwd").is_err());
    }

    #[test]
    fn validate_entry_path_rejects_empty() {
        assert!(validate_entry_path("").is_err());
        assert!(validate_entry_path(".").is_err());
        assert!(validate_entry_path("./.").is_err());
    }

    #[test]
    fn validate_hardlink_target_rejects_escape() {
        assert!(validate_hardlink_target("../bin/sh").is_err());
        assert!(validate_hardlink_target("/bin/sh").is_err());
        assert_eq!(
            validate_hardlink_target("bin/busybox").unwrap(),
            "bin/busybox"
        );
    }
}
