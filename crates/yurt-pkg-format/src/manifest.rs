//! Typed manifest structures matching the spec.
//!
//! `info/index.json`, `info/files.json`, and the optional `info/yurt.json`
//! deserialize directly into the structs in this module. Validation rules
//! that the spec calls out (canonical ownership, accepted file types, etc.)
//! live as `validate()` methods on the relevant types.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Initial schema version for both `info/index.json` and the registry index.
pub const SCHEMA_VERSION: u32 = 1;

/// Canonical Yurt ownership: root.
pub const CANONICAL_ROOT_UID: u32 = 0;
pub const CANONICAL_ROOT_GID: u32 = 0;

/// Canonical Yurt ownership: user.
pub const CANONICAL_USER_UID: u32 = 1000;
pub const CANONICAL_USER_GID: u32 = 1000;

/// `info/index.json` — required, describes package identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexManifest {
    pub schema_version: u32,
    pub name: String,
    pub version: String,
    pub build: String,
    pub platform: String,
    pub summary: String,
    pub license: String,
    #[serde(default)]
    pub depends: Vec<Depends>,
}

impl IndexManifest {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(Error::InvalidManifest(format!(
                "unsupported schema_version: {}",
                self.schema_version
            )));
        }
        if self.name.is_empty() {
            return Err(Error::InvalidManifest("name must not be empty".into()));
        }
        if self.name.to_lowercase() != self.name {
            return Err(Error::InvalidManifest(format!(
                "name must be lowercase: {}",
                self.name
            )));
        }
        if self.version.is_empty() {
            return Err(Error::InvalidManifest("version must not be empty".into()));
        }
        if self.build.is_empty() {
            return Err(Error::InvalidManifest("build must not be empty".into()));
        }
        if self.platform.is_empty() {
            return Err(Error::InvalidManifest("platform must not be empty".into()));
        }
        for dep in &self.depends {
            dep.validate()?;
        }
        Ok(())
    }

    /// Canonical artifact basename: `<name>-<version>-<build>.yurtpkg.tar.zst`.
    pub fn artifact_basename(&self) -> String {
        format!(
            "{}-{}-{}.yurtpkg.tar.zst",
            self.name, self.version, self.build
        )
    }
}

/// A dependency constraint.
///
/// First-cut serialization is the Conda-like single-string form, e.g.
/// `"libz >=1.3,<2"`. The string is preserved as-is for the future solver;
/// validation only enforces non-emptiness.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct Depends(pub String);

impl Depends {
    pub fn validate(&self) -> Result<()> {
        if self.0.trim().is_empty() {
            return Err(Error::InvalidManifest(
                "dependency entry must not be empty".into(),
            ));
        }
        Ok(())
    }

    /// Package name portion (everything before the first space, if any).
    pub fn name(&self) -> &str {
        self.0
            .split_once(char::is_whitespace)
            .map(|(n, _)| n)
            .unwrap_or(&self.0)
            .trim()
    }
}

/// `info/files.json` — required, records the installed file manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FilesManifest {
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    #[serde(rename = "type")]
    pub kind: FileEntryKind,
    /// sha256 hex digest. Required for regular files. Absent for symlinks
    /// and hardlinks (whose payload is the link target, not file bytes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// File size in bytes. Required for regular files. Absent for links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Symlink/hardlink target. Required for symlinks and hardlinks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Octal mode string, e.g. "0755".
    pub mode: String,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FileEntryKind {
    File,
    Dir,
    Symlink,
    Hardlink,
}

impl FileEntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FileEntryKind::File => "file",
            FileEntryKind::Dir => "dir",
            FileEntryKind::Symlink => "symlink",
            FileEntryKind::Hardlink => "hardlink",
        }
    }
}

impl FileEntry {
    pub fn validate(&self) -> Result<()> {
        if self.path.is_empty() {
            return Err(Error::InvalidManifest(
                "file entry path must not be empty".into(),
            ));
        }
        // Mode parses as octal (the spec writes them as "0755" strings).
        u32::from_str_radix(self.mode.trim_start_matches('0'), 8).map_err(|_| {
            Error::InvalidManifest(format!("invalid mode '{}' on '{}'", self.mode, self.path))
        })?;
        match self.kind {
            FileEntryKind::File => {
                if self.sha256.is_none() {
                    return Err(Error::InvalidManifest(format!(
                        "regular file '{}' is missing sha256",
                        self.path
                    )));
                }
                if self.size.is_none() {
                    return Err(Error::InvalidManifest(format!(
                        "regular file '{}' is missing size",
                        self.path
                    )));
                }
            }
            FileEntryKind::Symlink | FileEntryKind::Hardlink => {
                if self.target.is_none() {
                    return Err(Error::InvalidManifest(format!(
                        "{} '{}' is missing target",
                        self.kind.as_str(),
                        self.path
                    )));
                }
            }
            FileEntryKind::Dir => {}
        }
        Ok(())
    }

    /// Mode bits parsed from the octal string representation.
    pub fn mode_bits(&self) -> u32 {
        u32::from_str_radix(self.mode.trim_start_matches('0'), 8).unwrap_or(0)
    }
}

impl FilesManifest {
    pub fn validate(&self) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        for entry in &self.files {
            entry.validate()?;
            if !seen.insert(entry.path.as_str()) {
                return Err(Error::DuplicateEntry(entry.path.clone()));
            }
        }
        Ok(())
    }
}

/// `info/yurt.json` — optional, runtime requirements that aren't filesystem facts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct YurtManifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_yurt_version: Option<String>,
    #[serde(default)]
    pub requires: RuntimeRequirements,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RuntimeRequirements {
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub processes: bool,
    #[serde(default)]
    pub threads: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good_index() -> IndexManifest {
        IndexManifest {
            schema_version: SCHEMA_VERSION,
            name: "busybox".into(),
            version: "1.36.1".into(),
            build: "yurt_0".into(),
            platform: "wasm32-wasip1-yurt".into(),
            summary: "BusyBox userland".into(),
            license: "GPL-2.0-only".into(),
            depends: vec![],
        }
    }

    #[test]
    fn index_validates_and_round_trips() {
        let m = good_index();
        m.validate().unwrap();
        let json = serde_json::to_string(&m).unwrap();
        let m2: IndexManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(m, m2);
    }

    #[test]
    fn index_artifact_basename_matches_spec() {
        let m = good_index();
        assert_eq!(
            m.artifact_basename(),
            "busybox-1.36.1-yurt_0.yurtpkg.tar.zst"
        );
    }

    #[test]
    fn index_rejects_uppercase_name() {
        let mut m = good_index();
        m.name = "BusyBox".into();
        assert!(m.validate().is_err());
    }

    #[test]
    fn index_rejects_unsupported_schema_version() {
        let mut m = good_index();
        m.schema_version = 999;
        assert!(m.validate().is_err());
    }

    #[test]
    fn depends_extracts_name() {
        let d = Depends("libz >=1.3,<2".into());
        assert_eq!(d.name(), "libz");
        let d2 = Depends("busybox".into());
        assert_eq!(d2.name(), "busybox");
    }

    #[test]
    fn depends_rejects_blank() {
        assert!(Depends("   ".into()).validate().is_err());
    }

    #[test]
    fn file_entry_requires_sha_and_size_for_files() {
        let mut e = FileEntry {
            path: "bin/cat".into(),
            kind: FileEntryKind::File,
            sha256: None,
            size: None,
            target: None,
            mode: "0755".into(),
            uid: 0,
            gid: 0,
        };
        assert!(e.validate().is_err());
        e.sha256 = Some("00".repeat(32));
        assert!(e.validate().is_err());
        e.size = Some(0);
        e.validate().unwrap();
    }

    #[test]
    fn file_entry_requires_target_for_symlink() {
        let mut e = FileEntry {
            path: "bin/sh".into(),
            kind: FileEntryKind::Symlink,
            sha256: None,
            size: None,
            target: None,
            mode: "0777".into(),
            uid: 0,
            gid: 0,
        };
        assert!(e.validate().is_err());
        e.target = Some("busybox".into());
        e.validate().unwrap();
    }

    #[test]
    fn files_manifest_rejects_duplicates() {
        let m = FilesManifest {
            files: vec![
                FileEntry {
                    path: "bin/cat".into(),
                    kind: FileEntryKind::File,
                    sha256: Some("ab".repeat(32)),
                    size: Some(1),
                    target: None,
                    mode: "0755".into(),
                    uid: 0,
                    gid: 0,
                },
                FileEntry {
                    path: "bin/cat".into(),
                    kind: FileEntryKind::File,
                    sha256: Some("ab".repeat(32)),
                    size: Some(1),
                    target: None,
                    mode: "0755".into(),
                    uid: 0,
                    gid: 0,
                },
            ],
        };
        assert!(m.validate().is_err());
    }

    #[test]
    fn mode_bits_parse_octal() {
        let e = FileEntry {
            path: "bin/cat".into(),
            kind: FileEntryKind::File,
            sha256: Some("ab".repeat(32)),
            size: Some(1),
            target: None,
            mode: "0755".into(),
            uid: 0,
            gid: 0,
        };
        assert_eq!(e.mode_bits(), 0o755);
    }
}
