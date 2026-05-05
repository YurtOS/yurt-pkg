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

/// Whether `(uid, gid)` is one of the two canonical Yurt ownership tuples.
/// Tools that build packages should warn (or refuse) on non-canonical
/// values, since the kernel only models these two users today.
pub fn is_canonical_ownership(uid: u32, gid: u32) -> bool {
    (uid == CANONICAL_ROOT_UID && gid == CANONICAL_ROOT_GID)
        || (uid == CANONICAL_USER_UID && gid == CANONICAL_USER_GID)
}

/// Validate a package name. Pinned ASCII charset to keep names
/// unambiguous across registries: `^[a-z0-9][a-z0-9._-]*$`. Public so
/// registry tooling and `yurt-pack name --check`-style commands can
/// reuse the same rule the format library enforces.
pub fn validate_package_name(name: &str) -> Result<()> {
    if !is_valid_package_name(name) {
        return Err(Error::InvalidManifest(if name.is_empty() {
            "package name must not be empty".into()
        } else {
            format!("invalid package name '{name}': must match ^[a-z0-9][a-z0-9._-]*$")
        }));
    }
    Ok(())
}

fn is_valid_package_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-')
}

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
        validate_package_name(&self.name)?;
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
    /// Canonical 4-character zero-padded octal mode, e.g. `"0755"`,
    /// `"0000"`, `"4755"`. Validated by `FileEntry::validate`; any
    /// other encoding is rejected so different tools produce the same
    /// bits for the same string.
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
        // Canonical mode encoding: 4-character zero-padded octal, e.g.
        // "0755", "0644", "0000". Reject anything that doesn't match so
        // every implementation that reads our archives produces the same
        // bits for the same string. Split the diagnostic so a stale
        // mode like "8755" doesn't get the unhelpful "wrong length"
        // complaint.
        if let Some(reason) = canonical_mode_string_error(&self.mode) {
            return Err(Error::InvalidManifest(format!(
                "invalid mode '{}' on '{}': {reason}",
                self.mode, self.path
            )));
        }
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

    /// Mode bits parsed from the canonical 4-character octal string.
    /// Returns an error if the string isn't canonical, so callers that
    /// skipped `validate()` can't silently fall back to mode 0.
    pub fn mode_bits(&self) -> Result<u32> {
        if let Some(reason) = canonical_mode_string_error(&self.mode) {
            return Err(Error::InvalidManifest(format!(
                "cannot parse mode '{}' on '{}': {reason}",
                self.mode, self.path
            )));
        }
        Ok(u32::from_str_radix(&self.mode, 8).expect("validated above"))
    }
}

/// Return `Some(reason)` if `s` is not the canonical 4-character
/// zero-padded octal mode representation. Splits the failure into
/// actionable diagnostics: wrong length vs. non-octal digit.
fn canonical_mode_string_error(s: &str) -> Option<&'static str> {
    if s.len() != 4 {
        return Some(
            "expected exactly 4 characters of zero-padded octal (e.g. '0644', '0000', '4755')",
        );
    }
    if !s.bytes().all(|b| b.is_ascii_digit() && b < b'8') {
        return Some("each character must be an octal digit (0-7)");
    }
    None
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
    fn index_rejects_unicode_lowercase_lookalikes() {
        // 'σ' lowercases to itself but is not in our ASCII charset.
        let mut m = good_index();
        m.name = "σysadmin".into();
        assert!(m.validate().is_err());
    }

    #[test]
    fn index_rejects_leading_punctuation_in_name() {
        let mut m = good_index();
        m.name = "-foo".into();
        assert!(m.validate().is_err());
        m.name = ".foo".into();
        assert!(m.validate().is_err());
    }

    #[test]
    fn index_accepts_valid_name_charset() {
        let mut m = good_index();
        for name in [
            "foo", "foo-bar", "foo.bar", "foo_bar", "0", "lib_z3", "py3.11",
        ] {
            m.name = name.into();
            m.validate()
                .unwrap_or_else(|e| panic!("rejected valid name '{name}': {e}"));
        }
    }

    #[test]
    fn is_canonical_ownership_matches_spec() {
        assert!(is_canonical_ownership(0, 0));
        assert!(is_canonical_ownership(1000, 1000));
        assert!(!is_canonical_ownership(1000, 0));
        assert!(!is_canonical_ownership(500, 500));
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
        assert_eq!(e.mode_bits().unwrap(), 0o755);
    }

    #[test]
    fn mode_bits_errors_on_non_canonical_input() {
        // Callers who skipped validate() must surface a structured
        // error instead of silently falling back to 0.
        let e = FileEntry {
            path: "bin/x".into(),
            kind: FileEntryKind::File,
            sha256: Some("00".repeat(32)),
            size: Some(0),
            target: None,
            mode: "0o755".into(),
            uid: 0,
            gid: 0,
        };
        assert!(e.mode_bits().is_err());
    }

    #[test]
    fn mode_zero_round_trips() {
        // Regression: previously `trim_start_matches('0')` on "0000" left
        // an empty string, which `from_str_radix` rejected. mode 0 is
        // legal (chmod 0 produces it) and must validate + parse to 0.
        let e = FileEntry {
            path: "bin/empty".into(),
            kind: FileEntryKind::File,
            sha256: Some("00".repeat(32)),
            size: Some(0),
            target: None,
            mode: "0000".into(),
            uid: 0,
            gid: 0,
        };
        e.validate().unwrap();
        assert_eq!(e.mode_bits().unwrap(), 0);
    }

    #[test]
    fn mode_setuid_round_trips() {
        // 4755 = setuid + 0755. The canonical-mode check must allow the
        // top-bit forms tar entries can carry.
        let e = FileEntry {
            path: "bin/su".into(),
            kind: FileEntryKind::File,
            sha256: Some("ab".repeat(32)),
            size: Some(1),
            target: None,
            mode: "4755".into(),
            uid: 0,
            gid: 0,
        };
        e.validate().unwrap();
        assert_eq!(e.mode_bits().unwrap(), 0o4755);
    }

    #[test]
    fn mode_rejects_non_canonical_encodings() {
        // Each non-canonical form must fail validation: too short,
        // missing leading zero, hex digits, or octal-out-of-range.
        for bad in ["755", "0", "00755", "0o644", "0xff", "0648"] {
            let e = FileEntry {
                path: "bin/x".into(),
                kind: FileEntryKind::File,
                sha256: Some("00".repeat(32)),
                size: Some(0),
                target: None,
                mode: bad.into(),
                uid: 0,
                gid: 0,
            };
            assert!(
                e.validate().is_err(),
                "expected '{bad}' to be rejected as non-canonical mode"
            );
        }
    }

    #[test]
    fn mode_diagnostic_distinguishes_length_from_octal_digit() {
        // Wrong-length and non-octal-digit are different mistakes;
        // each should get a message that points the author at the fix.
        let bad_length = FileEntry {
            path: "bin/x".into(),
            kind: FileEntryKind::File,
            sha256: Some("00".repeat(32)),
            size: Some(0),
            target: None,
            mode: "755".into(),
            uid: 0,
            gid: 0,
        };
        let err = bad_length.validate().unwrap_err();
        assert!(
            format!("{err}").contains("4 characters"),
            "wrong-length error should mention the 4-character requirement, got: {err}"
        );

        let bad_digit = FileEntry {
            mode: "8755".into(),
            ..bad_length
        };
        let err = bad_digit.validate().unwrap_err();
        assert!(
            format!("{err}").contains("0-7"),
            "non-octal-digit error should mention the 0-7 range, got: {err}"
        );
    }

    #[test]
    fn validate_package_name_is_public_and_structured() {
        // Public helper for registry tooling: same rule as
        // IndexManifest::validate, but callable in isolation.
        validate_package_name("busybox").unwrap();
        let err = validate_package_name("").unwrap_err();
        assert!(format!("{err}").contains("must not be empty"));
        let err = validate_package_name("BadName").unwrap_err();
        assert!(format!("{err}").contains("must match"));
    }
}
