//! Read and write `.yurtpkg.tar.zst` archives.
//!
//! The on-disk layout is:
//! ```text
//! info/index.json     (required)
//! info/files.json     (required)
//! info/yurt.json      (optional)
//! <payload entries>   (relative paths under sandbox root)
//! ```
//!
//! Both `Reader` and `Writer` enforce the spec's path-validation rules
//! at the archive boundary so callers can trust that anything coming out
//! of `Reader::entries()` has already been normalized.

use std::collections::HashSet;
use std::io::{Read, Write};

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::manifest::{FileEntry, FileEntryKind, FilesManifest, IndexManifest, YurtManifest};
use crate::path::{validate_entry_path, validate_hardlink_target};

/// One entry as parsed from the archive payload (i.e. excluding `info/*`).
#[derive(Debug)]
pub struct ArchiveEntry {
    pub path: String,
    pub kind: EntryKind,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Debug)]
pub enum EntryKind {
    File { content: Vec<u8> },
    Dir,
    Symlink { target: String },
    Hardlink { target: String },
}

impl EntryKind {
    pub fn as_file_entry_kind(&self) -> FileEntryKind {
        match self {
            EntryKind::File { .. } => FileEntryKind::File,
            EntryKind::Dir => FileEntryKind::Dir,
            EntryKind::Symlink { .. } => FileEntryKind::Symlink,
            EntryKind::Hardlink { .. } => FileEntryKind::Hardlink,
        }
    }
}

/// Read a `.yurtpkg.tar.zst` archive from any [`std::io::Read`] source.
#[derive(Debug)]
pub struct Reader {
    pub index: IndexManifest,
    pub files: FilesManifest,
    pub yurt: Option<YurtManifest>,
    pub entries: Vec<ArchiveEntry>,
}

impl Reader {
    /// Read, decompress, parse, and validate an archive in one shot.
    ///
    /// On success, all paths have been normalized, the manifest matches
    /// the payload (every regular file's sha256 and size verified), and
    /// no duplicate paths exist.
    pub fn read<R: Read>(src: R) -> Result<Reader> {
        let zstd = zstd::Decoder::new(src)?;
        let mut tar = tar::Archive::new(zstd);

        let mut index: Option<IndexManifest> = None;
        let mut files: Option<FilesManifest> = None;
        let mut yurt: Option<YurtManifest> = None;
        let mut entries: Vec<ArchiveEntry> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        for raw in tar.entries()? {
            let mut entry = raw?;
            let raw_path = entry
                .path()?
                .to_str()
                .ok_or_else(|| Error::InvalidPath {
                    path: "<non-utf8>".into(),
                    reason: "tar entry path is not valid UTF-8",
                })?
                .to_string();

            // info/* manifests are read separately and not surfaced as entries.
            // Each may appear at most once — a malicious archive that ships
            // two `info/index.json` blocks must not silently take the second.
            if raw_path == "info/index.json" {
                if index.is_some() {
                    return Err(Error::DuplicateEntry(raw_path));
                }
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)?;
                index = Some(serde_json::from_slice(&buf)?);
                continue;
            }
            if raw_path == "info/files.json" {
                if files.is_some() {
                    return Err(Error::DuplicateEntry(raw_path));
                }
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)?;
                files = Some(serde_json::from_slice(&buf)?);
                continue;
            }
            if raw_path == "info/yurt.json" {
                if yurt.is_some() {
                    return Err(Error::DuplicateEntry(raw_path));
                }
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)?;
                yurt = Some(serde_json::from_slice(&buf)?);
                continue;
            }

            let path = validate_entry_path(&raw_path)?;
            if !seen.insert(path.clone()) {
                return Err(Error::DuplicateEntry(path));
            }

            let header = entry.header().clone();
            let mode = header.mode().unwrap_or(0);
            // Tar uid/gid are u64 in the crate's API but our format
            // models u32 (matching POSIX), so refuse rather than
            // silently truncate values that won't round-trip.
            let uid = u32_from_tar_u64(header.uid().unwrap_or(0), &path, "uid")?;
            let gid = u32_from_tar_u64(header.gid().unwrap_or(0), &path, "gid")?;

            let kind = match header.entry_type() {
                tar::EntryType::Regular => {
                    let mut content = Vec::new();
                    entry.read_to_end(&mut content)?;
                    EntryKind::File { content }
                }
                tar::EntryType::Directory => EntryKind::Dir,
                tar::EntryType::Symlink => {
                    let target = header
                        .link_name()?
                        .ok_or_else(|| Error::UnsupportedEntry {
                            path: path.clone(),
                            kind: "symlink without target".into(),
                        })?
                        .to_str()
                        .ok_or_else(|| Error::InvalidPath {
                            path: path.clone(),
                            reason: "symlink target is not valid UTF-8",
                        })?
                        .to_string();
                    EntryKind::Symlink { target }
                }
                tar::EntryType::Link => {
                    let target = header
                        .link_name()?
                        .ok_or_else(|| Error::UnsupportedEntry {
                            path: path.clone(),
                            kind: "hardlink without target".into(),
                        })?
                        .to_str()
                        .ok_or_else(|| Error::InvalidPath {
                            path: path.clone(),
                            reason: "hardlink target is not valid UTF-8",
                        })?
                        .to_string();
                    let target = validate_hardlink_target(&target)?;
                    EntryKind::Hardlink { target }
                }
                other => {
                    return Err(Error::UnsupportedEntry {
                        path,
                        kind: format!("{:?}", other),
                    });
                }
            };

            entries.push(ArchiveEntry {
                path,
                kind,
                mode,
                uid,
                gid,
            });
        }

        let index = index.ok_or(Error::MissingManifest("info/index.json"))?;
        let files = files.ok_or(Error::MissingManifest("info/files.json"))?;

        index.validate()?;
        files.validate()?;

        verify_files_against_entries(&files, &entries)?;

        Ok(Reader {
            index,
            files,
            yurt,
            entries,
        })
    }
}

/// Cross-check an archive's payload against `info/files.json`.
///
/// The tar payload is the source of truth for filesystem state, but
/// `info/files.json` exists so an installer can record installed paths
/// and uninstall later. They must agree on every path, type, hash, size,
/// and ownership tuple.
fn verify_files_against_entries(files: &FilesManifest, entries: &[ArchiveEntry]) -> Result<()> {
    let mut by_path: std::collections::HashMap<&str, &FileEntry> = std::collections::HashMap::new();
    for f in &files.files {
        by_path.insert(f.path.as_str(), f);
    }

    let mut covered: HashSet<&str> = HashSet::new();
    for entry in entries {
        let claimed = by_path
            .get(entry.path.as_str())
            .ok_or_else(|| Error::UnmanifestedEntry(entry.path.clone()))?;
        let actual = entry.kind.as_file_entry_kind();
        if actual != claimed.kind {
            return Err(Error::EntryTypeMismatch {
                path: entry.path.clone(),
                expected: claimed.kind.as_str(),
                actual: actual.as_str(),
            });
        }
        match &entry.kind {
            EntryKind::File { content } => {
                let expected_size = claimed.size.unwrap_or(0);
                if content.len() as u64 != expected_size {
                    return Err(Error::SizeMismatch {
                        path: entry.path.clone(),
                        expected: expected_size,
                        actual: content.len() as u64,
                    });
                }
                let actual_hash = sha256_hex(content);
                let expected_hash = claimed.sha256.as_deref().unwrap_or("");
                if actual_hash != expected_hash {
                    return Err(Error::HashMismatch {
                        path: entry.path.clone(),
                        expected: expected_hash.into(),
                        actual: actual_hash,
                    });
                }
            }
            EntryKind::Symlink { target } | EntryKind::Hardlink { target } => {
                let expected_target = claimed.target.as_deref().unwrap_or("");
                if expected_target != target {
                    return Err(Error::InvalidManifest(format!(
                        "{} target mismatch on '{}': manifest says '{}', archive has '{}'",
                        claimed.kind.as_str(),
                        entry.path,
                        expected_target,
                        target,
                    )));
                }
            }
            EntryKind::Dir => {}
        }
        covered.insert(entry.path.as_str());
    }

    for f in &files.files {
        if !covered.contains(f.path.as_str()) {
            return Err(Error::ManifestEntryMissing(f.path.clone()));
        }
    }

    Ok(())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode(digest)
}

/// Convert a tar header u64 uid/gid into our u32-modeled field, or
/// reject the archive if the value would silently truncate.
fn u32_from_tar_u64(v: u64, path: &str, field: &'static str) -> Result<u32> {
    u32::try_from(v).map_err(|_| {
        Error::InvalidManifest(format!(
            "{field} {v} on '{path}' exceeds u32 (the package format models POSIX uid/gid)",
        ))
    })
}

/// Write a `.yurtpkg.tar.zst` archive.
///
/// The writer expects callers to add archive entries in any order; on
/// `finish()` it computes `info/files.json` automatically from what was
/// added so manifest and payload cannot disagree by construction.
pub struct Writer {
    index: IndexManifest,
    yurt: Option<YurtManifest>,
    file_entries: Vec<FileEntry>,
    pending: Vec<PendingEntry>,
    seen: HashSet<String>,
}

struct PendingEntry {
    path: String,
    mode: u32,
    uid: u32,
    gid: u32,
    body: PendingBody,
}

enum PendingBody {
    File(Vec<u8>),
    Dir,
    Symlink(String),
    Hardlink(String),
}

impl Writer {
    pub fn new(index: IndexManifest, yurt: Option<YurtManifest>) -> Result<Self> {
        index.validate()?;
        Ok(Self {
            index,
            yurt,
            file_entries: Vec::new(),
            pending: Vec::new(),
            seen: HashSet::new(),
        })
    }

    pub fn add_file(
        &mut self,
        path: &str,
        content: Vec<u8>,
        mode: u32,
        uid: u32,
        gid: u32,
    ) -> Result<()> {
        let path = self.normalize_and_track(path)?;
        let size = content.len() as u64;
        let sha = sha256_hex(&content);
        self.file_entries.push(FileEntry {
            path: path.clone(),
            kind: FileEntryKind::File,
            sha256: Some(sha),
            size: Some(size),
            target: None,
            mode: format!("{:04o}", mode & 0o7777),
            uid,
            gid,
        });
        self.pending.push(PendingEntry {
            path,
            mode,
            uid,
            gid,
            body: PendingBody::File(content),
        });
        Ok(())
    }

    pub fn add_dir(&mut self, path: &str, mode: u32, uid: u32, gid: u32) -> Result<()> {
        let path = self.normalize_and_track(path)?;
        self.file_entries.push(FileEntry {
            path: path.clone(),
            kind: FileEntryKind::Dir,
            sha256: None,
            size: None,
            target: None,
            mode: format!("{:04o}", mode & 0o7777),
            uid,
            gid,
        });
        self.pending.push(PendingEntry {
            path,
            mode,
            uid,
            gid,
            body: PendingBody::Dir,
        });
        Ok(())
    }

    pub fn add_symlink(
        &mut self,
        path: &str,
        target: &str,
        mode: u32,
        uid: u32,
        gid: u32,
    ) -> Result<()> {
        let path = self.normalize_and_track(path)?;
        self.file_entries.push(FileEntry {
            path: path.clone(),
            kind: FileEntryKind::Symlink,
            sha256: None,
            size: None,
            target: Some(target.to_string()),
            mode: format!("{:04o}", mode & 0o7777),
            uid,
            gid,
        });
        self.pending.push(PendingEntry {
            path,
            mode,
            uid,
            gid,
            body: PendingBody::Symlink(target.to_string()),
        });
        Ok(())
    }

    pub fn add_hardlink(
        &mut self,
        path: &str,
        target: &str,
        mode: u32,
        uid: u32,
        gid: u32,
    ) -> Result<()> {
        let path = self.normalize_and_track(path)?;
        let target = validate_hardlink_target(target)?;
        self.file_entries.push(FileEntry {
            path: path.clone(),
            kind: FileEntryKind::Hardlink,
            sha256: None,
            size: None,
            target: Some(target.clone()),
            mode: format!("{:04o}", mode & 0o7777),
            uid,
            gid,
        });
        self.pending.push(PendingEntry {
            path,
            mode,
            uid,
            gid,
            body: PendingBody::Hardlink(target),
        });
        Ok(())
    }

    fn normalize_and_track(&mut self, path: &str) -> Result<String> {
        let path = validate_entry_path(path)?;
        if !self.seen.insert(path.clone()) {
            return Err(Error::DuplicateEntry(path));
        }
        Ok(path)
    }

    /// Flush all entries, write `info/*` manifests, and finalize the
    /// zstd stream into `dst`.
    pub fn finish<W: Write>(self, dst: W) -> Result<()> {
        let zstd = zstd::Encoder::new(dst, 0)?.auto_finish();
        let mut tar = tar::Builder::new(zstd);

        let files = FilesManifest {
            files: self.file_entries.clone(),
        };
        let index_json = serde_json::to_vec_pretty(&self.index)?;
        let files_json = serde_json::to_vec_pretty(&files)?;

        write_info(&mut tar, "info/index.json", &index_json)?;
        write_info(&mut tar, "info/files.json", &files_json)?;
        if let Some(y) = &self.yurt {
            let y_json = serde_json::to_vec_pretty(y)?;
            write_info(&mut tar, "info/yurt.json", &y_json)?;
        }

        for entry in self.pending {
            let mut header = tar::Header::new_gnu();
            header.set_mode(entry.mode);
            header.set_uid(entry.uid as u64);
            header.set_gid(entry.gid as u64);
            match entry.body {
                PendingBody::File(content) => {
                    header.set_entry_type(tar::EntryType::Regular);
                    header.set_size(content.len() as u64);
                    header.set_cksum();
                    tar.append_data(&mut header, &entry.path, content.as_slice())?;
                }
                PendingBody::Dir => {
                    header.set_entry_type(tar::EntryType::Directory);
                    header.set_size(0);
                    header.set_cksum();
                    tar.append_data(&mut header, &entry.path, std::io::empty())?;
                }
                PendingBody::Symlink(target) => {
                    header.set_entry_type(tar::EntryType::Symlink);
                    header.set_size(0);
                    header.set_link_name(&target)?;
                    header.set_cksum();
                    tar.append_data(&mut header, &entry.path, std::io::empty())?;
                }
                PendingBody::Hardlink(target) => {
                    header.set_entry_type(tar::EntryType::Link);
                    header.set_size(0);
                    header.set_link_name(&target)?;
                    header.set_cksum();
                    tar.append_data(&mut header, &entry.path, std::io::empty())?;
                }
            }
        }

        tar.finish()?;
        Ok(())
    }
}

fn write_info<W: Write>(tar: &mut tar::Builder<W>, name: &str, data: &[u8]) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(data.len() as u64);
    header.set_cksum();
    tar.append_data(&mut header, name, data)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{IndexManifest, RuntimeRequirements, YurtManifest, SCHEMA_VERSION};

    fn sample_index() -> IndexManifest {
        IndexManifest {
            schema_version: SCHEMA_VERSION,
            name: "demo".into(),
            version: "0.1.0".into(),
            build: "yurt_0".into(),
            platform: "wasm32-wasip1-yurt".into(),
            summary: "Demo package".into(),
            license: "Apache-2.0".into(),
            depends: vec![],
        }
    }

    fn sample_yurt() -> YurtManifest {
        YurtManifest {
            min_yurt_version: Some("0.1.0".into()),
            requires: RuntimeRequirements {
                network: false,
                processes: true,
                threads: false,
            },
            commands: vec!["demo".into()],
        }
    }

    #[test]
    fn round_trip_files_dirs_symlinks_hardlinks() {
        let mut w = Writer::new(sample_index(), Some(sample_yurt())).unwrap();
        w.add_dir("bin", 0o755, 0, 0).unwrap();
        w.add_file("bin/demo", b"hello".to_vec(), 0o755, 0, 0)
            .unwrap();
        w.add_symlink("bin/sh", "demo", 0o777, 0, 0).unwrap();
        w.add_hardlink("bin/demo2", "bin/demo", 0o755, 0, 0)
            .unwrap();

        let mut buf: Vec<u8> = Vec::new();
        w.finish(&mut buf).unwrap();

        let r = Reader::read(buf.as_slice()).unwrap();
        assert_eq!(r.index.name, "demo");
        assert_eq!(r.index.version, "0.1.0");
        assert!(r.yurt.is_some());
        assert_eq!(r.entries.len(), 4);
        assert_eq!(r.files.files.len(), 4);
    }

    #[test]
    fn duplicate_entry_path_rejected_at_write_time() {
        let mut w = Writer::new(sample_index(), None).unwrap();
        w.add_file("bin/demo", b"a".to_vec(), 0o755, 0, 0).unwrap();
        let err = w
            .add_file("bin/demo", b"b".to_vec(), 0o755, 0, 0)
            .unwrap_err();
        assert!(matches!(err, Error::DuplicateEntry(_)));
    }

    #[test]
    fn absolute_path_rejected_at_write_time() {
        let mut w = Writer::new(sample_index(), None).unwrap();
        let err = w
            .add_file("/etc/passwd", b"x".to_vec(), 0o644, 0, 0)
            .unwrap_err();
        assert!(matches!(err, Error::InvalidPath { .. }));
    }

    #[test]
    fn traversal_path_rejected_at_write_time() {
        let mut w = Writer::new(sample_index(), None).unwrap();
        let err = w
            .add_file("../etc/passwd", b"x".to_vec(), 0o644, 0, 0)
            .unwrap_err();
        assert!(matches!(err, Error::InvalidPath { .. }));
    }

    #[test]
    fn hardlink_target_traversal_rejected() {
        let mut w = Writer::new(sample_index(), None).unwrap();
        w.add_file("bin/demo", b"a".to_vec(), 0o755, 0, 0).unwrap();
        let err = w
            .add_hardlink("bin/demo2", "../../etc/passwd", 0o755, 0, 0)
            .unwrap_err();
        assert!(matches!(err, Error::InvalidPath { .. }));
    }

    #[test]
    fn read_rejects_missing_index_manifest() {
        // Build a tar.zst by hand that only contains a non-info entry.
        let mut buf: Vec<u8> = Vec::new();
        {
            let zstd = zstd::Encoder::new(&mut buf, 0).unwrap().auto_finish();
            let mut tar = tar::Builder::new(zstd);
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(1);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            tar.append_data(&mut header, "bin/demo", b"x".as_ref())
                .unwrap();
            tar.finish().unwrap();
        }
        let err = Reader::read(buf.as_slice()).unwrap_err();
        assert!(matches!(err, Error::MissingManifest("info/index.json")));
    }

    #[test]
    fn read_rejects_path_traversal_in_payload() {
        // The tar crate refuses to *write* a path containing `..`, so we
        // build a clean archive with a benign name and then patch the
        // entry's name in the decompressed tar bytes to a traversal path.
        // This is what a malicious archive produced by a non-validating
        // tool (or a hand-crafted attack) would look like, and
        // Reader::read must still reject it.
        //
        // The hardcoded byte offsets below (124 for size, 148 for the
        // checksum field) are the POSIX/GNU tar layout. They've been
        // stable for decades; if the tar crate ever switches default
        // formats this test needs to follow, but the rejection path it
        // exercises is what matters.
        let mut w = Writer::new(sample_index(), None).unwrap();
        w.add_file("escape", b"".to_vec(), 0o644, 0, 0).unwrap();
        let mut zst: Vec<u8> = Vec::new();
        w.finish(&mut zst).unwrap();

        // Decompress, find the payload entry's header, rewrite its name
        // field, recompute the checksum, recompress.
        let mut tar_bytes: Vec<u8> = Vec::new();
        zstd::Decoder::new(zst.as_slice())
            .unwrap()
            .read_to_end(&mut tar_bytes)
            .unwrap();
        let payload_offset = find_tar_header_offset(&tar_bytes, b"escape\0")
            .expect("payload entry not found in tar bytes");
        // Rewrite the 100-byte name field in place. Pad with NUL bytes.
        let new_name = b"../escape\0";
        for (i, slot) in tar_bytes[payload_offset..payload_offset + 100]
            .iter_mut()
            .enumerate()
        {
            *slot = if i < new_name.len() { new_name[i] } else { 0 };
        }
        recompute_tar_checksum(&mut tar_bytes[payload_offset..payload_offset + 512]);

        let mut patched: Vec<u8> = Vec::new();
        zstd::Encoder::new(&mut patched, 0)
            .unwrap()
            .auto_finish()
            .write_all(&tar_bytes)
            .unwrap();

        let err = Reader::read(patched.as_slice()).unwrap_err();
        assert!(
            matches!(err, Error::InvalidPath { .. }),
            "expected InvalidPath, got {:?}",
            err,
        );
    }

    /// Find the byte offset of a tar header whose name starts with the
    /// given needle. Tar headers are 512-byte blocks; the name field is
    /// the first 100 bytes.
    fn find_tar_header_offset(bytes: &[u8], needle: &[u8]) -> Option<usize> {
        let mut offset = 0;
        while offset + 512 <= bytes.len() {
            let name_field = &bytes[offset..offset + 100];
            if name_field.starts_with(needle) {
                return Some(offset);
            }
            // Skip past header + content (rounded up to 512).
            let size_field = &bytes[offset + 124..offset + 136];
            let size_str = std::str::from_utf8(size_field)
                .ok()?
                .trim_end_matches('\0')
                .trim_end_matches(' ');
            let size = u64::from_str_radix(size_str.trim(), 8).unwrap_or(0);
            let blocks = size.div_ceil(512);
            offset += 512 + (blocks * 512) as usize;
        }
        None
    }

    /// Recompute the POSIX tar header checksum (sum of all bytes with the
    /// checksum field treated as ASCII spaces), then write it back into
    /// the checksum field at offset 148.
    fn recompute_tar_checksum(header: &mut [u8]) {
        for slot in &mut header[148..156] {
            *slot = b' ';
        }
        let sum: u32 = header.iter().map(|&b| b as u32).sum();
        let s = format!("{:06o}\0 ", sum);
        header[148..156].copy_from_slice(s.as_bytes());
    }

    #[test]
    fn read_detects_hash_mismatch() {
        // Write a valid archive, then mutate `info/files.json` so the sha256 lies.
        let mut w = Writer::new(sample_index(), None).unwrap();
        w.add_file("bin/demo", b"hello".to_vec(), 0o755, 0, 0)
            .unwrap();
        let mut buf: Vec<u8> = Vec::new();
        w.finish(&mut buf).unwrap();

        // Decompress, mutate the JSON inside, recompress.
        let mutated = mutate_files_manifest(&buf, |files| {
            files.files[0].sha256 = Some("ff".repeat(32));
        });
        let err = Reader::read(mutated.as_slice()).unwrap_err();
        assert!(matches!(err, Error::HashMismatch { .. }));
    }

    #[test]
    fn duplicate_archive_path_after_normalization_rejected() {
        // "bin/demo" and "./bin/demo" both normalize to "bin/demo". The
        // spec says: "Reject duplicate archive entries after path
        // normalization." Verify that the writer's normalize-and-track
        // catches this even when the input strings differ.
        let mut w = Writer::new(sample_index(), None).unwrap();
        w.add_file("bin/demo", b"a".to_vec(), 0o755, 0, 0).unwrap();
        let err = w
            .add_file("./bin/demo", b"b".to_vec(), 0o755, 0, 0)
            .unwrap_err();
        assert!(matches!(err, Error::DuplicateEntry(_)));
    }

    #[test]
    fn round_trip_preserves_non_zero_uid_gid() {
        // Every other test uses (0, 0); the format also supports the
        // canonical user (1000, 1000) and any arbitrary u32. Verify the
        // tar header ←→ FileEntry path keeps the values intact.
        let mut w = Writer::new(sample_index(), None).unwrap();
        w.add_file("home/user/file.txt", b"hi".to_vec(), 0o644, 1000, 1000)
            .unwrap();
        let mut buf: Vec<u8> = Vec::new();
        w.finish(&mut buf).unwrap();

        let r = Reader::read(buf.as_slice()).unwrap();
        let e = &r.entries[0];
        assert_eq!(e.uid, 1000);
        assert_eq!(e.gid, 1000);
        assert_eq!(r.files.files[0].uid, 1000);
        assert_eq!(r.files.files[0].gid, 1000);
    }

    #[test]
    fn read_rejects_missing_files_manifest() {
        // Build an archive with info/index.json present but
        // info/files.json missing. Reader must surface the second
        // missing-manifest error, distinct from the index-missing case.
        let mut buf: Vec<u8> = Vec::new();
        {
            let zstd = zstd::Encoder::new(&mut buf, 0).unwrap().auto_finish();
            let mut tar = tar::Builder::new(zstd);
            let idx_json = serde_json::to_vec(&sample_index()).unwrap();
            super::write_info(&mut tar, "info/index.json", &idx_json).unwrap();
            tar.finish().unwrap();
        }
        let err = Reader::read(buf.as_slice()).unwrap_err();
        assert!(matches!(err, Error::MissingManifest("info/files.json")));
    }

    #[test]
    fn read_rejects_duplicate_info_index_manifest() {
        // A malicious archive ships two `info/index.json` blocks. The
        // first parse must stick and the second must error rather than
        // silently overwrite.
        let mut buf: Vec<u8> = Vec::new();
        {
            let zstd = zstd::Encoder::new(&mut buf, 0).unwrap().auto_finish();
            let mut tar = tar::Builder::new(zstd);
            let idx_json = serde_json::to_vec(&sample_index()).unwrap();
            super::write_info(&mut tar, "info/index.json", &idx_json).unwrap();
            super::write_info(&mut tar, "info/index.json", &idx_json).unwrap();
            tar.finish().unwrap();
        }
        let err = Reader::read(buf.as_slice()).unwrap_err();
        assert!(
            matches!(&err, Error::DuplicateEntry(p) if p == "info/index.json"),
            "expected DuplicateEntry, got {:?}",
            err,
        );
    }

    #[test]
    fn read_rejects_duplicate_info_files_manifest() {
        // Symmetric with the index-manifest test: the same defense
        // applies to info/files.json.
        let mut buf: Vec<u8> = Vec::new();
        {
            let zstd = zstd::Encoder::new(&mut buf, 0).unwrap().auto_finish();
            let mut tar = tar::Builder::new(zstd);
            let idx_json = serde_json::to_vec(&sample_index()).unwrap();
            let files_json = serde_json::to_vec(&FilesManifest { files: vec![] }).unwrap();
            super::write_info(&mut tar, "info/index.json", &idx_json).unwrap();
            super::write_info(&mut tar, "info/files.json", &files_json).unwrap();
            super::write_info(&mut tar, "info/files.json", &files_json).unwrap();
            tar.finish().unwrap();
        }
        let err = Reader::read(buf.as_slice()).unwrap_err();
        assert!(
            matches!(&err, Error::DuplicateEntry(p) if p == "info/files.json"),
            "expected DuplicateEntry, got {:?}",
            err,
        );
    }

    #[test]
    fn read_rejects_duplicate_info_yurt_manifest() {
        // Same defense for the optional info/yurt.json.
        let mut buf: Vec<u8> = Vec::new();
        {
            let zstd = zstd::Encoder::new(&mut buf, 0).unwrap().auto_finish();
            let mut tar = tar::Builder::new(zstd);
            let idx_json = serde_json::to_vec(&sample_index()).unwrap();
            let files_json = serde_json::to_vec(&FilesManifest { files: vec![] }).unwrap();
            let yurt_json = serde_json::to_vec(&sample_yurt()).unwrap();
            super::write_info(&mut tar, "info/index.json", &idx_json).unwrap();
            super::write_info(&mut tar, "info/files.json", &files_json).unwrap();
            super::write_info(&mut tar, "info/yurt.json", &yurt_json).unwrap();
            super::write_info(&mut tar, "info/yurt.json", &yurt_json).unwrap();
            tar.finish().unwrap();
        }
        let err = Reader::read(buf.as_slice()).unwrap_err();
        assert!(
            matches!(&err, Error::DuplicateEntry(p) if p == "info/yurt.json"),
            "expected DuplicateEntry, got {:?}",
            err,
        );
    }

    /// Test helper: round-trip an archive while letting the caller mutate
    /// `info/files.json` on the way through.
    fn mutate_files_manifest(input: &[u8], mutate: impl FnOnce(&mut FilesManifest)) -> Vec<u8> {
        let zstd = zstd::Decoder::new(input).unwrap();
        let mut tar = tar::Archive::new(zstd);

        // Buffer everything so we can rewrite only the files.json entry.
        struct Captured {
            header: tar::Header,
            path: String,
            data: Vec<u8>,
            link: Option<String>,
        }
        let mut captured: Vec<Captured> = Vec::new();
        for raw in tar.entries().unwrap() {
            let mut entry = raw.unwrap();
            let header = entry.header().clone();
            let path = entry.path().unwrap().to_string_lossy().into_owned();
            let link = header
                .link_name()
                .ok()
                .flatten()
                .map(|p| p.to_string_lossy().into_owned());
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            captured.push(Captured {
                header,
                path,
                data,
                link,
            });
        }

        // Pre-compute the rewritten files.json so the FnOnce only fires once.
        let files_idx = captured
            .iter()
            .position(|c| c.path == "info/files.json")
            .expect("input archive lacks info/files.json");
        let mut files: FilesManifest = serde_json::from_slice(&captured[files_idx].data).unwrap();
        mutate(&mut files);
        let rewritten_files_json = serde_json::to_vec(&files).unwrap();

        let mut out: Vec<u8> = Vec::new();
        {
            let zstd = zstd::Encoder::new(&mut out, 0).unwrap().auto_finish();
            let mut builder = tar::Builder::new(zstd);
            for (i, c) in captured.into_iter().enumerate() {
                if i == files_idx {
                    let mut header = tar::Header::new_gnu();
                    header.set_mode(0o644);
                    header.set_size(rewritten_files_json.len() as u64);
                    header.set_entry_type(tar::EntryType::Regular);
                    header.set_cksum();
                    builder
                        .append_data(&mut header, &c.path, rewritten_files_json.as_slice())
                        .unwrap();
                } else {
                    let mut header = c.header;
                    if let Some(link) = &c.link {
                        header.set_link_name(link).unwrap();
                    }
                    header.set_cksum();
                    builder
                        .append_data(&mut header, &c.path, c.data.as_slice())
                        .unwrap();
                }
            }
            builder.finish().unwrap();
        }
        out
    }
}
