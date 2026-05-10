use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use yurt_pkg_format::{EntryKind, FileEntryKind, Reader};
use yurt_pkg_repo::fetch::{FetchRequest, FetchResponse, LocalFileFetcher, RepoFetcher};
#[cfg(not(any(test, feature = "test-fixtures")))]
use yurt_pkg_repo::verify::{BundleVerifier, VerificationInput};
#[cfg(not(any(test, feature = "test-fixtures")))]
use yurt_pkg_trust::TrustRoot;

use crate::installed::{InstalledPackageInput, InstalledStore};
use crate::resolver::PackageRecord;

pub fn apply_plan(
    root: &Path,
    state_root: &Path,
    store: &InstalledStore,
    packages: &[PackageRecord],
) -> Result<()> {
    let archives = packages
        .iter()
        .map(load_archive)
        .collect::<Result<Vec<_>>>()?;
    check_transaction_collisions(root, store, &archives)?;

    let txid = transaction_id();
    let staging_root = state_root.join("staging").join(&txid).join("root");
    if staging_root.exists() {
        fs::remove_dir_all(&staging_root)
            .with_context(|| format!("failed to clear {}", staging_root.display()))?;
    }
    fs::create_dir_all(&staging_root)
        .with_context(|| format!("failed to create {}", staging_root.display()))?;

    for archive in &archives {
        write_entries(&staging_root, &archive.reader)
            .with_context(|| format!("failed to stage {}", archive.package.name))?;
    }
    let inputs = archives
        .iter()
        .map(installed_input)
        .collect::<Result<Vec<_>>>()?;
    store.prepare_install(&txid, &inputs)?;

    for archive in &archives {
        write_entries(root, &archive.reader)
            .with_context(|| format!("failed to install {}", archive.package.name))?;
    }

    store.mark_prepared_committed(&txid)?;
    let staging_tx = state_root.join("staging").join(&txid);
    if staging_tx.exists() {
        fs::remove_dir_all(&staging_tx)
            .with_context(|| format!("failed to remove {}", staging_tx.display()))?;
    }
    Ok(())
}

struct LoadedArchive<'a> {
    package: &'a PackageRecord,
    reader: Reader,
}

fn load_archive(package: &PackageRecord) -> Result<LoadedArchive<'_>> {
    let url = url::Url::parse(&package.url)
        .with_context(|| format!("invalid archive url for {}", package.name))?;
    let response = LocalFileFetcher
        .fetch(FetchRequest {
            url: &url,
            etag: None,
            credential_origin: None,
        })
        .with_context(|| format!("failed to fetch {}", package.url))?;
    let FetchResponse::Modified { body, .. } = response else {
        bail!(
            "archive fetch for {} unexpectedly returned not-modified",
            package.name
        );
    };
    if body.len() as u64 != package.size {
        bail!(
            "archive size mismatch for {} {}-{}",
            package.name,
            package.version,
            package.build
        );
    }
    let actual_hash = hex(&Sha256::digest(&body));
    if actual_hash != package.sha256 {
        bail!(
            "archive hash mismatch for {} {}-{}",
            package.name,
            package.version,
            package.build
        );
    }
    let bundle = fetch_bundle(&url)?;
    verify_bundle(package, &body, &bundle)?;
    let reader = Reader::read(body.as_slice())
        .with_context(|| format!("failed to read archive for {}", package.name))?;
    verify_archive_metadata(package, &reader)?;
    Ok(LoadedArchive { package, reader })
}

fn fetch_bundle(url: &url::Url) -> Result<Vec<u8>> {
    let bundle_url = url::Url::parse(&format!("{url}.bundle"))
        .with_context(|| format!("invalid archive bundle url for {url}"))?;
    let response = LocalFileFetcher
        .fetch(FetchRequest {
            url: &bundle_url,
            etag: None,
            credential_origin: None,
        })
        .with_context(|| format!("failed to fetch {}", bundle_url))?;
    let FetchResponse::Modified { body, .. } = response else {
        bail!("bundle fetch for {bundle_url} unexpectedly returned not-modified");
    };
    Ok(body)
}

#[cfg(any(test, feature = "test-fixtures"))]
fn verify_bundle(package: &PackageRecord, payload: &[u8], bundle: &[u8]) -> Result<()> {
    let _ = payload;
    if !cfg!(feature = "test-fixtures")
        && std::env::var_os("YURT_PKG_TEST_STATIC_ARCHIVE_VERIFIER").is_none()
    {
        bail!("bundle verification is not wired to sigstore yet");
    }
    if bundle.is_empty() {
        bail!(
            "archive signature verification failed for {} {}-{}",
            package.name,
            package.version,
            package.build
        );
    }
    if package.signing.subject.is_empty() || package.signing.issuer.is_empty() {
        bail!(
            "archive signature verification failed for {} {}-{}",
            package.name,
            package.version,
            package.build
        );
    }
    Ok(())
}

#[cfg(not(any(test, feature = "test-fixtures")))]
fn verify_bundle(package: &PackageRecord, payload: &[u8], bundle: &[u8]) -> Result<()> {
    yurt_pkg_repo::verify::NotImplementedVerifier
        .verify(VerificationInput {
            payload,
            bundle,
            expected_signing: &package.signing,
            trust_root: &TrustRoot::from_dir("/etc/yurt-pkg/sigstore-trust-root"),
        })
        .with_context(|| {
            format!(
                "archive signature verification failed for {} {}-{}",
                package.name, package.version, package.build
            )
        })?;
    Ok(())
}

fn verify_archive_metadata(package: &PackageRecord, reader: &Reader) -> Result<()> {
    if reader.index.name != package.name
        || reader.index.version != package.version
        || reader.index.build != package.build
    {
        bail!(
            "archive metadata mismatch for {} {}-{}",
            package.name,
            package.version,
            package.build
        );
    }
    let expected = dependency_map(&package.depends)?;
    let actual = dependency_map(&reader.index.depends)?;
    if expected != actual {
        bail!("archive dependency metadata mismatch for {}", package.name);
    }
    Ok(())
}

fn dependency_map(deps: &[yurt_pkg_format::Depends]) -> Result<BTreeMap<String, String>> {
    deps.iter()
        .map(|dep| {
            let req = semver::VersionReq::parse(&dep.req)
                .with_context(|| format!("invalid dependency {} {}", dep.name, dep.req))?;
            Ok((dep.name.clone(), req.to_string()))
        })
        .collect()
}

fn check_transaction_collisions(
    root: &Path,
    store: &InstalledStore,
    archives: &[LoadedArchive<'_>],
) -> Result<()> {
    let mut directory_paths = BTreeSet::new();
    let mut non_directory_paths = BTreeSet::new();
    for archive in archives {
        for file in &archive.reader.files.files {
            match file.kind {
                FileEntryKind::Dir => {
                    // In-transaction reverse: an earlier archive in the
                    // same plan placed a non-directory at this exact path.
                    // The post-loop sweep below catches this too, but
                    // failing inline gives the dir-side ownership info
                    // when the error is raised.
                    if non_directory_paths.contains(&file.path) {
                        bail!(
                            "{} {}-{} would replace non-directory path {} \
                             with a directory in the same transaction",
                            archive.package.name,
                            archive.package.version,
                            archive.package.build,
                            file.path
                        );
                    }
                    // Already-on-disk collision. Existing path that is
                    // *also* a directory is fine (multiple packages
                    // routinely claim ownership of /usr, /usr/lib, ...).
                    // Existing path that is a non-directory is the bug
                    // the reviewer flagged: prepare_install would write
                    // its row, then write_entries would call
                    // create_dir_all on a regular-file path and explode,
                    // leaving the staging tree wedged.
                    let destination = root.join(&file.path);
                    if let Some(metadata) = symlink_metadata_opt(&destination)? {
                        if !metadata.is_dir() {
                            // Enrich the error with the owning package
                            // when one is registered in the installed
                            // store; fall back to "unmanaged" otherwise.
                            // path_owner doesn't carry kind, but the
                            // filesystem agrees with the committed-
                            // installed state for any committed entry,
                            // so the kind check above is sufficient.
                            if let Some(owner) = store.path_owner(&file.path)? {
                                bail!(
                                    "{} {}-{} would replace path {} \
                                     (currently a non-directory owned by {}) \
                                     with a directory",
                                    archive.package.name,
                                    archive.package.version,
                                    archive.package.build,
                                    file.path,
                                    owner
                                );
                            }
                            bail!(
                                "{} {}-{} would replace unmanaged \
                                 non-directory path {} with a directory",
                                archive.package.name,
                                archive.package.version,
                                archive.package.build,
                                file.path
                            );
                        }
                    }
                    directory_paths.insert(file.path.clone());
                }
                FileEntryKind::File | FileEntryKind::Symlink | FileEntryKind::Hardlink => {
                    if directory_paths.contains(&file.path) {
                        bail!("{} collides with directory path {}", file.path, file.path);
                    }
                    if !non_directory_paths.insert(file.path.clone()) {
                        bail!("path {} is owned by multiple packages", file.path);
                    }
                    if let Some(owner) = store.path_owner(&file.path)? {
                        bail!(
                            "{} {}-{} would overwrite path {} owned by {}",
                            archive.package.name,
                            archive.package.version,
                            archive.package.build,
                            file.path,
                            owner
                        );
                    }
                    let destination = root.join(&file.path);
                    if symlink_metadata_exists(&destination)? {
                        bail!("would overwrite unmanaged path {}", file.path);
                    }
                }
            }
        }
    }
    for path in &non_directory_paths {
        if directory_paths.contains(path) {
            bail!("{path} collides with directory path {path}");
        }
    }
    Ok(())
}

fn write_entries(root: &Path, reader: &Reader) -> Result<()> {
    for entry in &reader.entries {
        let path = root.join(&entry.path);
        match &entry.kind {
            EntryKind::Dir => {
                fs::create_dir_all(&path)
                    .with_context(|| format!("failed to create {}", path.display()))?;
                set_mode(&path, entry.mode)?;
            }
            EntryKind::File { content } => {
                ensure_parent(&path)?;
                fs::write(&path, content)
                    .with_context(|| format!("failed to write {}", path.display()))?;
                set_mode(&path, entry.mode)?;
            }
            EntryKind::Symlink { target } => {
                ensure_parent(&path)?;
                remove_existing_link_path(&path)?;
                symlink(target, &path)
                    .with_context(|| format!("failed to symlink {}", path.display()))?;
            }
            EntryKind::Hardlink { target } => {
                ensure_parent(&path)?;
                remove_existing_link_path(&path)?;
                fs::hard_link(root.join(target), &path)
                    .with_context(|| format!("failed to hardlink {}", path.display()))?;
            }
        }
    }
    Ok(())
}

fn installed_input(archive: &LoadedArchive<'_>) -> Result<InstalledPackageInput> {
    Ok(InstalledPackageInput {
        name: archive.package.name.clone(),
        version: archive.package.version.clone(),
        build: archive.package.build.clone(),
        repo_id: archive.package.repo_id.clone(),
        source_url: archive.package.url.clone(),
        sha256: archive.package.sha256.clone(),
        size: archive.package.size,
        index_json: serde_json::to_string(&archive.reader.index)
            .context("failed to serialize installed index manifest")?,
        files: archive.reader.files.files.clone(),
        dependencies: archive.reader.index.depends.clone(),
        yurt_json: archive
            .reader
            .yurt
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .context("failed to serialize installed yurt manifest")?,
    })
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    Ok(())
}

fn remove_existing_link_path(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => fs::remove_file(path)
            .with_context(|| format!("failed to replace {}", path.display()))?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(anyhow!(err).context(format!("failed to inspect {}", path.display())))
        }
    }
    Ok(())
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777))
        .with_context(|| format!("failed to chmod {}", path.display()))
}

fn symlink_metadata_exists(path: &Path) -> Result<bool> {
    Ok(symlink_metadata_opt(path)?.is_some())
}

fn symlink_metadata_opt(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(anyhow!(err).context(format!("failed to inspect {}", path.display()))),
    }
}

fn transaction_id() -> String {
    format!(
        "tx-{}-{}",
        std::process::id(),
        time::OffsetDateTime::now_utc().unix_timestamp_nanos()
    )
}

fn hex(bytes: &[u8]) -> String {
    const CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(CHARS[(byte >> 4) as usize] as char);
        out.push(CHARS[(byte & 0xf) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installed::InstalledStore;
    use crate::resolver::PackageRecord;
    use sha2::{Digest, Sha256};
    use tempfile::{tempdir, TempDir};
    use yurt_pkg_format::{Depends, IndexManifest, Writer};
    use yurt_pkg_trust::SigningIdentity;

    #[test]
    fn applies_archive_and_records_file_owner() {
        std::env::set_var("YURT_PKG_TEST_STATIC_ARCHIVE_VERIFIER", "1");
        let state = tempdir().unwrap();
        let root = tempdir().unwrap();
        let archive_dir = tempdir().unwrap();
        let store = InstalledStore::open(state.path()).unwrap();
        let package = planned_package_with_archive(
            &archive_dir,
            "hello",
            "1.0.0",
            "yurt_0",
            archive_with_file("hello", "1.0.0", "yurt_0", "bin/hello", b"hello\n"),
        );

        apply_plan(root.path(), state.path(), &store, &[package]).unwrap();

        assert_eq!(
            std::fs::read(root.path().join("bin/hello")).unwrap(),
            b"hello\n"
        );
        assert_eq!(store.path_owner("bin/hello").unwrap().unwrap(), "hello");
    }

    #[test]
    fn rejects_dir_over_installed_file() {
        // Reverse of the existing collision case: package A is already
        // installed and owns share/foo as a regular file; package B
        // arrives in a fresh transaction with share/foo as a directory.
        // Without the dir-arm guard, prepare_install would write its row
        // and then write_entries would call create_dir_all on the on-disk
        // file path, leaving the staging tree wedged.
        std::env::set_var("YURT_PKG_TEST_STATIC_ARCHIVE_VERIFIER", "1");
        let state = tempdir().unwrap();
        let root = tempdir().unwrap();
        let archive_dir = tempdir().unwrap();
        let store = InstalledStore::open(state.path()).unwrap();

        // Phase 1: install package A with a file at share/foo.
        let pkg_a = planned_package_with_archive(
            &archive_dir,
            "a",
            "1.0.0",
            "yurt_0",
            archive_with_file("a", "1.0.0", "yurt_0", "share/foo", b"x"),
        );
        apply_plan(root.path(), state.path(), &store, &[pkg_a]).unwrap();
        assert_eq!(store.path_owner("share/foo").unwrap().unwrap(), "a");

        // Phase 2: try to install package B with a dir at share/foo.
        let pkg_b = planned_package_with_archive(
            &archive_dir,
            "b",
            "1.0.0",
            "yurt_0",
            archive_with_dir("b", "1.0.0", "yurt_0", "share/foo"),
        );
        let err = apply_plan(root.path(), state.path(), &store, &[pkg_b])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("would replace path share/foo")
                && err.contains("owned by a"),
            "unexpected error: {err}"
        );
        // Pre-prepare bail: nothing about pkg b should have been
        // written into the installed store.
        assert!(store.path_owner("share/foo").unwrap().as_deref() == Some("a"));
    }

    #[test]
    fn rejects_dir_over_unmanaged_file() {
        // Pre-existing on-disk file outside any package — e.g. a
        // hand-edited config or a leftover from a non-yurt install.
        // A package that ships a directory at the same path must
        // refuse to proceed.
        std::env::set_var("YURT_PKG_TEST_STATIC_ARCHIVE_VERIFIER", "1");
        let state = tempdir().unwrap();
        let root = tempdir().unwrap();
        let archive_dir = tempdir().unwrap();
        let store = InstalledStore::open(state.path()).unwrap();

        std::fs::create_dir_all(root.path().join("share")).unwrap();
        std::fs::write(root.path().join("share/foo"), b"unmanaged").unwrap();

        let pkg = planned_package_with_archive(
            &archive_dir,
            "b",
            "1.0.0",
            "yurt_0",
            archive_with_dir("b", "1.0.0", "yurt_0", "share/foo"),
        );
        let err = apply_plan(root.path(), state.path(), &store, &[pkg])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("unmanaged non-directory path share/foo"),
            "unexpected error: {err}"
        );
        // The unmanaged file must be untouched.
        assert_eq!(
            std::fs::read(root.path().join("share/foo")).unwrap(),
            b"unmanaged"
        );
    }

    #[test]
    fn allows_dir_over_existing_dir() {
        // Multiple packages routinely claim ownership of the same
        // directory (/usr, /usr/lib, /usr/share). A pre-existing
        // directory at a path a new package's dir entry occupies is
        // not a collision — only a non-directory there is.
        std::env::set_var("YURT_PKG_TEST_STATIC_ARCHIVE_VERIFIER", "1");
        let state = tempdir().unwrap();
        let root = tempdir().unwrap();
        let archive_dir = tempdir().unwrap();
        let store = InstalledStore::open(state.path()).unwrap();

        let pkg_a = planned_package_with_archive(
            &archive_dir,
            "a",
            "1.0.0",
            "yurt_0",
            archive_with_dir("a", "1.0.0", "yurt_0", "share/common"),
        );
        apply_plan(root.path(), state.path(), &store, &[pkg_a]).unwrap();

        let pkg_b = planned_package_with_archive(
            &archive_dir,
            "b",
            "1.0.0",
            "yurt_0",
            archive_with_dir("b", "1.0.0", "yurt_0", "share/common"),
        );
        // Should NOT bail — directories can co-occupy.
        apply_plan(root.path(), state.path(), &store, &[pkg_b]).unwrap();
    }

    #[test]
    fn rejects_file_directory_collision() {
        std::env::set_var("YURT_PKG_TEST_STATIC_ARCHIVE_VERIFIER", "1");
        let state = tempdir().unwrap();
        let root = tempdir().unwrap();
        let archive_dir = tempdir().unwrap();
        let store = InstalledStore::open(state.path()).unwrap();
        let packages = vec![
            planned_package_with_archive(
                &archive_dir,
                "a",
                "1.0.0",
                "yurt_0",
                archive_with_dir("a", "1.0.0", "yurt_0", "share/foo"),
            ),
            planned_package_with_archive(
                &archive_dir,
                "b",
                "1.0.0",
                "yurt_0",
                archive_with_file("b", "1.0.0", "yurt_0", "share/foo", b"x"),
            ),
        ];

        let err = apply_plan(root.path(), state.path(), &store, &packages)
            .unwrap_err()
            .to_string();

        assert!(err.contains("collides with directory path share/foo"));
    }

    fn archive_with_file(
        name: &str,
        version: &str,
        build: &str,
        path: &str,
        content: &[u8],
    ) -> Vec<u8> {
        let mut writer = Writer::new(index(name, version, build, &[]), None).unwrap();
        writer
            .add_file(path, content.to_vec(), 0o755, 0, 0)
            .unwrap();
        finish(writer)
    }

    fn archive_with_dir(name: &str, version: &str, build: &str, path: &str) -> Vec<u8> {
        let mut writer = Writer::new(index(name, version, build, &[]), None).unwrap();
        writer.add_dir(path, 0o755, 0, 0).unwrap();
        finish(writer)
    }

    fn finish(writer: Writer) -> Vec<u8> {
        let mut bytes = Vec::new();
        writer.finish(&mut bytes).unwrap();
        bytes
    }

    fn index(name: &str, version: &str, build: &str, depends: &[Depends]) -> IndexManifest {
        IndexManifest {
            schema_version: yurt_pkg_format::SCHEMA_VERSION,
            name: name.to_string(),
            version: version.to_string(),
            build: build.to_string(),
            platform: "wasm32-wasip1".to_string(),
            summary: String::new(),
            license: "Apache-2.0".to_string(),
            depends: depends.to_vec(),
        }
    }

    fn planned_package_with_archive(
        dir: &TempDir,
        name: &str,
        version: &str,
        build: &str,
        archive: Vec<u8>,
    ) -> PackageRecord {
        let path = dir.path().join(format!("{name}-{version}-{build}.yurtpkg"));
        std::fs::write(&path, &archive).unwrap();
        std::fs::write(path.with_extension("yurtpkg.bundle"), b"bundle").unwrap();
        PackageRecord {
            repo_id: "official".to_string(),
            priority: 0,
            name: name.to_string(),
            version: version.to_string(),
            build: build.to_string(),
            url: url::Url::from_file_path(&path).unwrap().to_string(),
            sha256: hex(&Sha256::digest(&archive)),
            size: archive.len() as u64,
            signing: SigningIdentity {
                subject: "subject".to_string(),
                issuer: "issuer".to_string(),
            },
            depends: Vec::new(),
            yanked: false,
        }
    }

    fn hex(bytes: &[u8]) -> String {
        const CHARS: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            out.push(CHARS[(byte >> 4) as usize] as char);
            out.push(CHARS[(byte & 0xf) as usize] as char);
        }
        out
    }
}
