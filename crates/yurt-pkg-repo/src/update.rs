//! Repository update engine.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;
use time::OffsetDateTime;
use url::Url;
use yurt_pkg_trust::{TrustRoot, TrustedRepo};

use crate::fetch::{FetchRequest, FetchResponse, RepoFetcher};
use crate::metadata::{Freshness, Index, PackageFile, RepoPackage};
use crate::search_index::RepoSearchIndex;
use crate::state::{RepoState, SnapshotManifest, TrustChange};
use crate::store::{LockMode, RepoCacheStore};
use crate::verify::{BundleVerifier, VerificationInput};

#[derive(Debug, Error)]
pub enum Error {
    #[error("fetch failed: {0}")]
    Fetch(#[from] crate::fetch::Error),
    #[error("verification failed: {0}")]
    Verify(#[from] crate::verify::Error),
    #[error("metadata validation failed: {0}")]
    Metadata(#[from] crate::metadata::Error),
    #[error("store failed: {0}")]
    Store(#[from] crate::store::Error),
    #[error("search index failed: {0}")]
    SearchIndex(#[from] crate::search_index::Error),
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("json error at {path}: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("index fetch returned 304 but no current snapshot exists")]
    NotModifiedWithoutSnapshot,
    #[error("modified index cannot use a stale or missing bundle")]
    MissingModifiedBundle,
    #[error("package '{package}' hash mismatch: expected {expected}, got {actual}")]
    PackageHashMismatch {
        package: String,
        expected: String,
        actual: String,
    },
    #[error("package '{package}' size mismatch: expected {expected}, got {actual}")]
    PackageSizeMismatch {
        package: String,
        expected: u64,
        actual: u64,
    },
    #[error("package file for key '{key}' declares name '{actual}'")]
    PackageNameMismatch { key: String, actual: String },
    #[error("Rekor integrated time rollback: new {new_time} is older than cached {cached_time}")]
    RekorRollback {
        new_time: OffsetDateTime,
        cached_time: OffsetDateTime,
    },
    #[error("staging directory already exists: {0}")]
    StagingExists(PathBuf),
    #[error("unsupported package entry url scheme: {0}")]
    UnsupportedPackageUrlScheme(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy)]
pub struct UpdateOptions {
    pub now: OffsetDateTime,
    pub freshness: Freshness,
}

#[derive(Debug, Clone)]
pub struct UpdateEngine<F, V> {
    pub fetcher: F,
    pub verifier: V,
    pub trust_root: TrustRoot,
    pub cache_store: RepoCacheStore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoUpdateOutcome {
    pub repo_id: String,
    pub changed: bool,
    pub index_version: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
struct CurrentSnapshot {
    snapshot_id: String,
    manifest: SnapshotManifest,
    index: Index,
    package_entries: BTreeMap<String, RepoPackage>,
}

impl<F, V> UpdateEngine<F, V>
where
    F: RepoFetcher,
    V: BundleVerifier,
{
    pub fn update_repo(
        &self,
        repo: &TrustedRepo,
        options: UpdateOptions,
    ) -> Result<RepoUpdateOutcome> {
        let _lock = self.cache_store.lock(&repo.id, LockMode::Exclusive)?;
        self.cache_store
            .repair_state_if_needed(&repo.id, options.now)?;
        let mut fetch_started = false;
        let result = self.update_repo_locked(repo, options, &mut fetch_started);
        if result.is_err() && fetch_started {
            self.increment_failure_count(repo, options.now)?;
        }
        result
    }

    fn update_repo_locked(
        &self,
        repo: &TrustedRepo,
        options: UpdateOptions,
        fetch_started: &mut bool,
    ) -> Result<RepoUpdateOutcome> {
        let state = self.cache_store.read_state(&repo.id)?;
        let current = self.read_current_snapshot(&repo.id)?;
        let trust_change = current
            .as_ref()
            .map(|current| current.manifest.trust_change(repo));

        let can_reuse_fetch = matches!(trust_change, Some(TrustChange::Unchanged));
        let can_reuse_packages = can_reuse_fetch;
        let enforce_baseline = !matches!(trust_change, Some(TrustChange::SigningIdentity));
        let index_url = repo
            .url
            .join("index.json")
            .map_err(|source| crate::metadata::Error::InvalidUrl(repo.id.clone(), source))?;
        *fetch_started = true;
        let index_response = self.fetcher.fetch(FetchRequest {
            url: &index_url,
            etag: state
                .as_ref()
                .and_then(|state| state.index_etag.as_deref())
                .filter(|_| can_reuse_fetch),
            credential_origin: Some(&repo.url),
        })?;

        let FetchResponse::Modified {
            body: index_bytes,
            etag: index_etag,
        } = index_response
        else {
            let Some(current) = current else {
                return Err(Error::NotModifiedWithoutSnapshot);
            };
            current
                .index
                .validate_against(None, options.now, options.freshness)?;
            self.cache_store.write_state(
                &repo.id,
                &RepoState {
                    schema: 1,
                    repo_id: repo.id.clone(),
                    current_snapshot: current.snapshot_id,
                    index_etag: state.as_ref().and_then(|state| state.index_etag.clone()),
                    index_bundle_etag: state
                        .as_ref()
                        .and_then(|state| state.index_bundle_etag.clone()),
                    last_fetched: options.now,
                    consecutive_fetch_failures: 0,
                },
            )?;
            return Ok(RepoUpdateOutcome {
                repo_id: repo.id.clone(),
                changed: false,
                index_version: current.index.index_version,
                warnings: Vec::new(),
            });
        };

        let bundle_url = repo
            .url
            .join("index.json.bundle")
            .map_err(|source| crate::metadata::Error::InvalidUrl(repo.id.clone(), source))?;
        let bundle_response = self.fetcher.fetch(FetchRequest {
            url: &bundle_url,
            etag: state
                .as_ref()
                .and_then(|state| state.index_bundle_etag.as_deref())
                .filter(|_| can_reuse_fetch),
            credential_origin: Some(&repo.url),
        })?;
        let FetchResponse::Modified {
            body: bundle_bytes,
            etag: index_bundle_etag,
        } = bundle_response
        else {
            return Err(Error::MissingModifiedBundle);
        };

        let verification = self.verifier.verify(VerificationInput {
            payload: &index_bytes,
            bundle: &bundle_bytes,
            expected_signing: &repo.signing,
            trust_root: &self.trust_root,
        })?;
        let index: Index = serde_json::from_slice(&index_bytes).map_err(|source| Error::Json {
            path: PathBuf::from("index.json"),
            source,
        })?;
        let cached_version = current
            .as_ref()
            .filter(|_| enforce_baseline)
            .map(|current| current.manifest.index_version);
        index.validate_against(cached_version, options.now, options.freshness)?;
        if let Some(current) = current.as_ref().filter(|_| enforce_baseline) {
            if verification.integrated_time < current.manifest.integrated_time {
                return Err(Error::RekorRollback {
                    new_time: verification.integrated_time,
                    cached_time: current.manifest.integrated_time,
                });
            }
        }

        let snapshot_id =
            RepoCacheStore::snapshot_id(options.now, index.index_version, &index_bytes);
        let staging = self.cache_store.staging_dir(&repo.id, &snapshot_id);
        if staging.exists() {
            return Err(Error::StagingExists(staging));
        }
        fs::create_dir_all(staging.join("packages")).map_err(|source| Error::Io {
            path: staging.join("packages"),
            source,
        })?;
        fs::write(staging.join("index.json"), &index_bytes).map_err(|source| Error::Io {
            path: staging.join("index.json"),
            source,
        })?;
        fs::write(staging.join("index.json.bundle"), &bundle_bytes).map_err(|source| {
            Error::Io {
                path: staging.join("index.json.bundle"),
                source,
            }
        })?;

        let packages = self.persist_package_files(
            repo,
            &index,
            current.as_ref(),
            &staging,
            can_reuse_packages,
        )?;
        let manifest = SnapshotManifest::from_verified_index(repo, &index, &verification);
        write_json(&staging.join("manifest.json"), &manifest)?;
        RepoSearchIndex::rebuild(staging.join("db.sqlite"), &repo.id, &packages)?;
        self.cache_store
            .commit_staging(&repo.id, &staging, &snapshot_id)?;
        self.cache_store.write_state(
            &repo.id,
            &RepoState {
                schema: 1,
                repo_id: repo.id.clone(),
                current_snapshot: snapshot_id,
                index_etag,
                index_bundle_etag,
                last_fetched: options.now,
                consecutive_fetch_failures: 0,
            },
        )?;

        Ok(RepoUpdateOutcome {
            repo_id: repo.id.clone(),
            changed: true,
            index_version: index.index_version,
            warnings: Vec::new(),
        })
    }

    fn increment_failure_count(&self, repo: &TrustedRepo, now: OffsetDateTime) -> Result<()> {
        let state = self.cache_store.read_state(&repo.id)?;
        let current_snapshot = self
            .cache_store
            .current_snapshot_id(&repo.id)?
            .or_else(|| state.as_ref().map(|state| state.current_snapshot.clone()))
            .unwrap_or_default();
        let mut state = state.unwrap_or_else(|| {
            RepoState::without_etags_for_repair(repo.id.clone(), current_snapshot.clone(), now)
        });
        state.current_snapshot = current_snapshot;
        state.last_fetched = now;
        state.consecutive_fetch_failures += 1;
        self.cache_store.write_state(&repo.id, &state)?;
        Ok(())
    }

    fn read_current_snapshot(&self, repo_id: &str) -> Result<Option<CurrentSnapshot>> {
        let Some(snapshot_id) = self.cache_store.current_snapshot_id(repo_id)? else {
            return Ok(None);
        };
        let dir = self.cache_store.snapshot_dir(repo_id, &snapshot_id);
        let manifest: SnapshotManifest = read_json(&dir.join("manifest.json"))?;
        let index: Index = read_json(&dir.join("index.json"))?;
        Ok(Some(CurrentSnapshot {
            snapshot_id,
            package_entries: index.packages.clone(),
            manifest,
            index,
        }))
    }

    fn persist_package_files(
        &self,
        repo: &TrustedRepo,
        index: &Index,
        current: Option<&CurrentSnapshot>,
        staging: &Path,
        can_reuse_packages: bool,
    ) -> Result<Vec<PackageFile>> {
        let mut packages = Vec::new();
        for (name, entry) in &index.packages {
            let target = staging.join("packages").join(format!("{name}.json"));
            let bytes = if can_reuse_packages
                && current
                    .and_then(|current| current.package_entries.get(name))
                    .is_some_and(|old| old.sha256 == entry.sha256 && old.size == entry.size)
            {
                let current = current.expect("checked above");
                fs::read(
                    self.cache_store
                        .snapshot_dir(&repo.id, &current.snapshot_id)
                        .join("packages")
                        .join(format!("{name}.json")),
                )
                .map_err(|source| Error::Io {
                    path: target.clone(),
                    source,
                })?
            } else {
                let url = resolve_package_url(&repo.url, entry)?;
                let credential_origin = if same_origin(&repo.url, &url) {
                    Some(&repo.url)
                } else {
                    None
                };
                match self.fetcher.fetch(FetchRequest {
                    url: &url,
                    etag: None,
                    credential_origin,
                })? {
                    FetchResponse::Modified { body, .. } => body,
                    FetchResponse::NotModified => return Err(Error::MissingModifiedBundle),
                }
            };
            verify_package_bytes(name, entry, &bytes)?;
            let package: PackageFile =
                serde_json::from_slice(&bytes).map_err(|source| Error::Json {
                    path: target.clone(),
                    source,
                })?;
            package.validate()?;
            if package.name != *name {
                return Err(Error::PackageNameMismatch {
                    key: name.clone(),
                    actual: package.name,
                });
            }
            fs::write(&target, &bytes).map_err(|source| Error::Io {
                path: target,
                source,
            })?;
            packages.push(package);
        }
        Ok(packages)
    }
}

pub fn resolve_package_url(base: &Url, entry: &RepoPackage) -> Result<Url> {
    if let Ok(url) = Url::parse(&entry.url) {
        if !matches!(url.scheme(), "file" | "http" | "https") {
            return Err(Error::UnsupportedPackageUrlScheme(url.scheme().to_string()));
        }
        return Ok(url);
    }
    base.join(&entry.url)
        .map_err(|source| crate::metadata::Error::InvalidUrl(entry.url.clone(), source).into())
}

pub fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

fn verify_package_bytes(name: &str, entry: &RepoPackage, bytes: &[u8]) -> Result<()> {
    let actual = hex::encode(Sha256::digest(bytes));
    if actual != entry.sha256 {
        return Err(Error::PackageHashMismatch {
            package: name.to_string(),
            expected: entry.sha256.clone(),
            actual,
        });
    }
    if bytes.len() as u64 != entry.size {
        return Err(Error::PackageSizeMismatch {
            package: name.to_string(),
            expected: entry.size,
            actual: bytes.len() as u64,
        });
    }
    Ok(())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| Error::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|source| Error::Json {
        path: path.to_path_buf(),
        source,
    })?;
    fs::write(path, bytes).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}
