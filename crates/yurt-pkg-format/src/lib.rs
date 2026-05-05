//! Reader, validator, and writer for the `.yurtpkg.tar.zst` package format.
//!
//! The format is normatively defined in
//! `yurtos-kernel/docs/superpowers/specs/2026-05-05-yurt-package-format-design.md`.
//!
//! This crate is intentionally side-effect free with respect to the host
//! filesystem: callers supply readers and writers, and the crate parses,
//! validates, and produces the on-disk byte layout.

pub mod archive;
pub mod error;
pub mod manifest;
pub mod path;

pub use archive::{sha256_hex, ArchiveEntry, EntryKind, Reader, Writer};
pub use error::{Error, Result};
pub use manifest::{
    is_canonical_ownership, validate_package_name, Depends, FileEntry, FileEntryKind,
    FilesManifest, IndexManifest, RuntimeRequirements, YurtManifest, CANONICAL_ROOT_GID,
    CANONICAL_ROOT_UID, CANONICAL_USER_GID, CANONICAL_USER_UID, SCHEMA_VERSION,
};
