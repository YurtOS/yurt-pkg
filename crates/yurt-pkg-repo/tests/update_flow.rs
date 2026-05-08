use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::rc::Rc;

use sha2::{Digest, Sha256};
use tempfile::tempdir;
use time::macros::datetime;
use time::{Duration, OffsetDateTime};
use url::Url;
use yurt_pkg_format::Depends;
use yurt_pkg_repo::fetch::{FetchRequest, FetchResponse, MemoryFetcher, RepoFetcher};
use yurt_pkg_repo::metadata::{Freshness, Index, PackageFile, PackageVersion, RepoPackage};
use yurt_pkg_repo::search_index::RepoSearchIndex;
use yurt_pkg_repo::state::RepoState;
use yurt_pkg_repo::store::RepoCacheStore;
use yurt_pkg_repo::update::{Error, UpdateEngine, UpdateOptions};
use yurt_pkg_repo::verify::{StaticVerifier, VerificationOutput};
use yurt_pkg_trust::{SigningIdentity, TrustRoot, TrustedRepo};

fn now() -> OffsetDateTime {
    datetime!(2026-05-08 12:00 UTC)
}

fn repo(url: &str) -> TrustedRepo {
    TrustedRepo {
        id: "official".to_string(),
        url: Url::parse(url).unwrap(),
        signing: SigningIdentity {
            subject: "subject".to_string(),
            issuer: "issuer".to_string(),
        },
        priority: 0,
    }
}

fn package_file(name: &str, version: &str) -> PackageFile {
    PackageFile {
        name: name.to_string(),
        versions: vec![PackageVersion {
            name: None,
            version: version.to_string(),
            build: "yurt_0".to_string(),
            url: format!("https://example.com/{name}.yurtpkg"),
            sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .to_string(),
            size: 1,
            signing: SigningIdentity {
                subject: "subject".to_string(),
                issuer: "issuer".to_string(),
            },
            depends: vec![Depends {
                name: "libc".to_string(),
                req: "^0.1".to_string(),
            }],
            yanked: false,
            yanked_reason: None,
        }],
    }
}

fn package_bytes(name: &str, version: &str) -> Vec<u8> {
    serde_json::to_vec(&package_file(name, version)).unwrap()
}

fn repo_package(bytes: &[u8], url: &str) -> RepoPackage {
    RepoPackage {
        sha256: hex::encode(Sha256::digest(bytes)),
        size: bytes.len() as u64,
        url: url.to_string(),
    }
}

fn index_bytes(version: u64, packages: BTreeMap<String, RepoPackage>) -> Vec<u8> {
    serde_json::to_vec(&Index {
        schema: 1,
        index_version: version,
        generated_at: now(),
        expires_at: now() + Duration::days(7),
        packages,
    })
    .unwrap()
}

fn verifier() -> StaticVerifier {
    verifier_for("subject", "issuer", now())
}

fn verifier_for(subject: &str, issuer: &str, integrated_time: OffsetDateTime) -> StaticVerifier {
    StaticVerifier {
        output: VerificationOutput {
            integrated_time,
            subject: subject.to_string(),
            issuer: issuer.to_string(),
        },
    }
}

fn options() -> UpdateOptions {
    UpdateOptions {
        now: now(),
        freshness: Freshness::default(),
    }
}

fn trust_root() -> TrustRoot {
    TrustRoot::from_dir("/tmp/yurt-pkg-test-trust-root")
}

fn fetcher_with_index(repo: &TrustedRepo, index: Vec<u8>, package_url: &str, package: Vec<u8>) -> MemoryFetcher {
    let mut fetcher = MemoryFetcher::default();
    fetcher.insert(repo.url.join("index.json").unwrap(), index, None);
    fetcher.insert(
        repo.url.join("index.json.bundle").unwrap(),
        b"bundle".to_vec(),
        None,
    );
    fetcher.insert(repo.url.join(package_url).unwrap(), package, None);
    fetcher
}

#[test]
fn update_fetches_signed_index_and_rebuilds_search_index() {
    let temp = tempdir().unwrap();
    let repo = repo("file:///repo/");
    let tool = package_bytes("tool", "1.0.0");
    let index = index_bytes(
        1,
        BTreeMap::from([("tool".to_string(), repo_package(&tool, "packages/tool.json"))]),
    );
    let fetcher = fetcher_with_index(&repo, index, "packages/tool.json", tool);
    let store = RepoCacheStore::new(temp.path());
    let engine = UpdateEngine {
        fetcher,
        verifier: verifier(),
        trust_root: trust_root(),
        cache_store: store.clone(),
    };

    let outcome = engine.update_repo(&repo, options()).unwrap();
    assert!(outcome.changed);
    let snapshot = store.current_snapshot_id("official").unwrap().unwrap();
    let index = RepoSearchIndex::new(store.snapshot_dir("official", &snapshot).join("db.sqlite"));
    let rows = index.search_local("tool").unwrap();
    assert_eq!(rows[0].name, "tool");
}

#[test]
fn relative_package_url_resolves_against_repo_base() {
    let temp = tempdir().unwrap();
    let repo = repo("file:///repo/");
    let tool = package_bytes("tool", "1.0.0");
    let index = index_bytes(
        1,
        BTreeMap::from([("tool".to_string(), repo_package(&tool, "pkg/tool-v1.json"))]),
    );
    let fetcher = fetcher_with_index(&repo, index, "pkg/tool-v1.json", tool);
    let store = RepoCacheStore::new(temp.path());
    let engine = UpdateEngine {
        fetcher,
        verifier: verifier(),
        trust_root: trust_root(),
        cache_store: store.clone(),
    };

    engine.update_repo(&repo, options()).unwrap();
    let snapshot = store.current_snapshot_id("official").unwrap().unwrap();
    assert!(store
        .snapshot_dir("official", &snapshot)
        .join("packages/tool.json")
        .exists());
}

#[test]
fn rollback_uses_current_manifest_index_version() {
    let temp = tempdir().unwrap();
    let repo = repo("file:///repo/");
    update_one(&temp, &repo, 5, "1.0.0").unwrap();

    for version in [4, 5] {
        let err = update_one(&temp, &repo, version, "1.0.1").unwrap_err();
        assert!(matches!(err, Error::Metadata(_)), "{err:?}");
    }
}

#[test]
fn package_name_must_match_index_key() {
    let temp = tempdir().unwrap();
    let repo = repo("file:///repo/");
    let other = package_bytes("other", "1.0.0");
    let index = index_bytes(
        1,
        BTreeMap::from([("tool".to_string(), repo_package(&other, "packages/tool.json"))]),
    );
    let fetcher = fetcher_with_index(&repo, index, "packages/tool.json", other);
    let engine = UpdateEngine {
        fetcher,
        verifier: verifier(),
        trust_root: trust_root(),
        cache_store: RepoCacheStore::new(temp.path()),
    };

    let err = engine.update_repo(&repo, options()).unwrap_err();
    assert!(matches!(err, Error::PackageNameMismatch { .. }));
}

#[test]
fn unchanged_package_files_are_carried_forward_to_new_snapshot() {
    let temp = tempdir().unwrap();
    let repo = repo("file:///repo/");
    update_one(&temp, &repo, 1, "1.0.0").unwrap();

    let tool = package_bytes("tool", "1.0.0");
    let new_tool = package_bytes("new-tool", "1.0.0");
    let index = index_bytes(
        2,
        BTreeMap::from([
            ("tool".to_string(), repo_package(&tool, "packages/tool.json")),
            (
                "new-tool".to_string(),
                repo_package(&new_tool, "packages/new-tool.json"),
            ),
        ]),
    );
    let mut fetcher = MemoryFetcher::default();
    fetcher.insert(repo.url.join("index.json").unwrap(), index, Some("index-2".into()));
    fetcher.insert(repo.url.join("index.json.bundle").unwrap(), b"bundle".to_vec(), None);
    fetcher.insert(repo.url.join("packages/new-tool.json").unwrap(), new_tool, None);
    let store = RepoCacheStore::new(temp.path());
    let engine = UpdateEngine {
        fetcher,
        verifier: verifier(),
        trust_root: trust_root(),
        cache_store: store.clone(),
    };

    engine.update_repo(&repo, options()).unwrap();
    let snapshot = store.current_snapshot_id("official").unwrap().unwrap();
    let dir = store.snapshot_dir("official", &snapshot).join("packages");
    assert!(dir.join("tool.json").exists());
    assert!(dir.join("new-tool.json").exists());
}

#[test]
fn removed_package_files_are_omitted_from_new_snapshot() {
    let temp = tempdir().unwrap();
    let repo = repo("file:///repo/");
    update_one(&temp, &repo, 1, "1.0.0").unwrap();

    let other = package_bytes("other", "1.0.0");
    let index = index_bytes(
        2,
        BTreeMap::from([("other".to_string(), repo_package(&other, "packages/other.json"))]),
    );
    let fetcher = fetcher_with_index(&repo, index, "packages/other.json", other);
    let store = RepoCacheStore::new(temp.path());
    let engine = UpdateEngine {
        fetcher,
        verifier: verifier(),
        trust_root: trust_root(),
        cache_store: store.clone(),
    };

    engine.update_repo(&repo, options()).unwrap();
    let snapshot = store.current_snapshot_id("official").unwrap().unwrap();
    let dir = store.snapshot_dir("official", &snapshot).join("packages");
    assert!(dir.join("other.json").exists());
    assert!(!dir.join("tool.json").exists());
}

#[test]
fn changed_package_file_replaces_previous_snapshot_copy() {
    let temp = tempdir().unwrap();
    let repo = repo("file:///repo/");
    update_one(&temp, &repo, 1, "1.0.0").unwrap();
    update_one(&temp, &repo, 2, "1.1.0").unwrap();
    let store = RepoCacheStore::new(temp.path());
    let snapshot = store.current_snapshot_id("official").unwrap().unwrap();
    let index = RepoSearchIndex::new(store.snapshot_dir("official", &snapshot).join("db.sqlite"));
    let info = index.info_local("tool").unwrap().unwrap();
    assert_eq!(info.package.versions[0].version, "1.1.0");
}

#[test]
fn not_modified_revalidates_cached_expiry_and_updates_last_fetched() {
    let temp = tempdir().unwrap();
    let repo = repo("file:///repo/");
    update_one(&temp, &repo, 1, "1.0.0").unwrap();
    let store = RepoCacheStore::new(temp.path());
    let old_snapshot = store.current_snapshot_id("official").unwrap().unwrap();
    let later = UpdateOptions {
        now: now() + Duration::hours(1),
        freshness: Freshness::default(),
    };
    let engine = UpdateEngine {
        fetcher: NotModifiedFetcher,
        verifier: verifier(),
        trust_root: trust_root(),
        cache_store: store.clone(),
    };

    let outcome = engine.update_repo(&repo, later).unwrap();
    assert!(!outcome.changed);
    assert_eq!(store.current_snapshot_id("official").unwrap().unwrap(), old_snapshot);
    assert_eq!(
        store.read_state("official").unwrap().unwrap().last_fetched,
        later.now
    );
}

#[test]
fn rekor_time_uses_current_manifest_integrated_time() {
    let temp = tempdir().unwrap();
    let repo = repo("file:///repo/");
    update_one(&temp, &repo, 1, "1.0.0").unwrap();
    let tool = package_bytes("tool", "1.1.0");
    let index = index_bytes(
        2,
        BTreeMap::from([("tool".to_string(), repo_package(&tool, "packages/tool.json"))]),
    );
    let fetcher = fetcher_with_index(&repo, index, "packages/tool.json", tool);
    let engine = UpdateEngine {
        fetcher,
        verifier: verifier_for("subject", "issuer", now() - Duration::days(1)),
        trust_root: trust_root(),
        cache_store: RepoCacheStore::new(temp.path()),
    };

    let err = engine.update_repo(&repo, options()).unwrap_err();
    assert!(matches!(err, Error::RekorRollback { .. }));
}

#[test]
fn url_only_change_keeps_rollback_protection() {
    let temp = tempdir().unwrap();
    let old_repo = repo("file:///repo/");
    update_one(&temp, &old_repo, 5, "1.0.0").unwrap();
    let new_repo = repo("file:///mirror/");
    let tool = package_bytes("tool", "1.0.1");
    let index = index_bytes(
        4,
        BTreeMap::from([("tool".to_string(), repo_package(&tool, "packages/tool.json"))]),
    );
    let fetcher = fetcher_with_index(&new_repo, index, "packages/tool.json", tool);
    let engine = UpdateEngine {
        fetcher,
        verifier: verifier(),
        trust_root: trust_root(),
        cache_store: RepoCacheStore::new(temp.path()),
    };

    let err = engine.update_repo(&new_repo, options()).unwrap_err();
    assert!(matches!(err, Error::Metadata(_)));
}

#[test]
fn signing_identity_change_resets_rollback_protection() {
    let temp = tempdir().unwrap();
    let mut old_repo = repo("file:///repo/");
    old_repo.signing.subject = "old".to_string();
    update_one_with_verifier(&temp, &old_repo, verifier_for("old", "issuer", now()), 5, "1.0.0")
        .unwrap();

    let new_repo = repo("file:///repo/");
    update_one(&temp, &new_repo, 1, "1.0.1").unwrap();
    let store = RepoCacheStore::new(temp.path());
    let manifest = store.read_current_manifest("official").unwrap().unwrap();
    assert_eq!(manifest.signing_subject, "subject");
    assert_eq!(manifest.index_version, 1);
}

#[test]
fn failed_update_preserves_previous_current_snapshot() {
    let temp = tempdir().unwrap();
    let repo = repo("file:///repo/");
    update_one(&temp, &repo, 1, "1.0.0").unwrap();
    let store = RepoCacheStore::new(temp.path());
    let old = store.current_snapshot_id("official").unwrap().unwrap();
    let tool = package_bytes("tool", "1.1.0");
    let mut bad_entry = repo_package(&tool, "packages/tool.json");
    bad_entry.sha256 = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string();
    let index = index_bytes(2, BTreeMap::from([("tool".to_string(), bad_entry)]));
    let fetcher = fetcher_with_index(&repo, index, "packages/tool.json", tool);
    let engine = UpdateEngine {
        fetcher,
        verifier: verifier(),
        trust_root: trust_root(),
        cache_store: store.clone(),
    };

    assert!(engine.update_repo(&repo, options()).is_err());
    assert_eq!(store.current_snapshot_id("official").unwrap().unwrap(), old);
}

#[test]
fn retried_same_index_version_after_abandoned_staging_does_not_collide() {
    let temp = tempdir().unwrap();
    let repo = repo("file:///repo/");
    let tool = package_bytes("tool", "1.0.0");
    let index = index_bytes(
        1,
        BTreeMap::from([("tool".to_string(), repo_package(&tool, "packages/tool.json"))]),
    );
    let orphan = RepoCacheStore::snapshot_id(now(), 1, &index);
    fs::create_dir_all(temp.path().join("official/snapshots").join(orphan)).unwrap();
    let fetcher = fetcher_with_index(&repo, index, "packages/tool.json", tool);
    let store = RepoCacheStore::new(temp.path());
    let engine = UpdateEngine {
        fetcher,
        verifier: verifier(),
        trust_root: trust_root(),
        cache_store: store.clone(),
    };

    engine.update_repo(&repo, options()).unwrap();
    assert!(store.current_snapshot_id("official").unwrap().is_some());
}

#[test]
fn failed_fetch_increments_failure_count_and_success_resets_it() {
    let temp = tempdir().unwrap();
    let repo = repo("file:///repo/");
    let store = RepoCacheStore::new(temp.path());
    let engine = UpdateEngine {
        fetcher: MemoryFetcher::default(),
        verifier: verifier(),
        trust_root: trust_root(),
        cache_store: store.clone(),
    };
    assert!(engine.update_repo(&repo, options()).is_err());
    assert_eq!(
        store
            .read_state("official")
            .unwrap()
            .unwrap()
            .consecutive_fetch_failures,
        1
    );

    update_one(&temp, &repo, 1, "1.0.0").unwrap();
    assert_eq!(
        store
            .read_state("official")
            .unwrap()
            .unwrap()
            .consecutive_fetch_failures,
        0
    );
}

#[test]
fn current_snapshot_repair_clears_stale_etags() {
    let temp = tempdir().unwrap();
    let repo = repo("file:///repo/");
    update_one(&temp, &repo, 1, "1.0.0").unwrap();
    let store = RepoCacheStore::new(temp.path());
    store
        .write_state(
            "official",
            &RepoState {
                schema: 1,
                repo_id: "official".to_string(),
                current_snapshot: "old".to_string(),
                index_etag: Some("stale".to_string()),
                index_bundle_etag: Some("stale".to_string()),
                last_fetched: now(),
                consecutive_fetch_failures: 0,
            },
        )
        .unwrap();

    update_one(&temp, &repo, 2, "1.0.1").unwrap();
    let state = store.read_state("official").unwrap().unwrap();
    assert_ne!(state.current_snapshot, "old");
    assert_eq!(state.index_etag, None);
}

#[test]
fn absolute_package_url_does_not_inherit_cross_origin_credentials() {
    let temp = tempdir().unwrap();
    let repo = repo("https://repo.example/index/");
    let tool = package_bytes("tool", "1.0.0");
    let absolute = "https://cdn.example/tool.json";
    let index = index_bytes(
        1,
        BTreeMap::from([("tool".to_string(), repo_package(&tool, absolute))]),
    );
    let fetcher = RecordingFetcher::new(vec![
        (repo.url.join("index.json").unwrap(), index),
        (repo.url.join("index.json.bundle").unwrap(), b"bundle".to_vec()),
        (Url::parse(absolute).unwrap(), tool),
    ]);
    let seen = fetcher.seen.clone();
    let engine = UpdateEngine {
        fetcher,
        verifier: verifier(),
        trust_root: trust_root(),
        cache_store: RepoCacheStore::new(temp.path()),
    };

    engine.update_repo(&repo, options()).unwrap();
    let seen = seen.borrow();
    assert!(seen.iter().any(|(url, origin)| {
        url.as_str() == absolute && origin.is_none()
    }));
}

fn update_one(
    temp: &tempfile::TempDir,
    repo: &TrustedRepo,
    version: u64,
    package_version: &str,
) -> Result<(), Error> {
    update_one_with_verifier(temp, repo, verifier(), version, package_version)
}

fn update_one_with_verifier(
    temp: &tempfile::TempDir,
    repo: &TrustedRepo,
    verifier: StaticVerifier,
    version: u64,
    package_version: &str,
) -> Result<(), Error> {
    let tool = package_bytes("tool", package_version);
    let index = index_bytes(
        version,
        BTreeMap::from([("tool".to_string(), repo_package(&tool, "packages/tool.json"))]),
    );
    let fetcher = fetcher_with_index(repo, index, "packages/tool.json", tool);
    let engine = UpdateEngine {
        fetcher,
        verifier,
        trust_root: trust_root(),
        cache_store: RepoCacheStore::new(temp.path()),
    };
    engine.update_repo(repo, options()).map(|_| ())
}

struct NotModifiedFetcher;

impl RepoFetcher for NotModifiedFetcher {
    fn fetch(&self, request: FetchRequest<'_>) -> yurt_pkg_repo::fetch::Result<FetchResponse> {
        if request.url.path().ends_with("index.json") {
            Ok(FetchResponse::NotModified)
        } else {
            Err(yurt_pkg_repo::fetch::Error::NotFound(request.url.clone()))
        }
    }
}

struct RecordingFetcher {
    entries: BTreeMap<Url, Vec<u8>>,
    seen: Rc<RefCell<Vec<(Url, Option<Url>)>>>,
}

impl RecordingFetcher {
    fn new(entries: Vec<(Url, Vec<u8>)>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
            seen: Rc::new(RefCell::new(Vec::new())),
        }
    }
}

impl RepoFetcher for RecordingFetcher {
    fn fetch(&self, request: FetchRequest<'_>) -> yurt_pkg_repo::fetch::Result<FetchResponse> {
        self.seen.borrow_mut().push((
            request.url.clone(),
            request.credential_origin.cloned(),
        ));
        let body = self
            .entries
            .get(request.url)
            .cloned()
            .ok_or_else(|| yurt_pkg_repo::fetch::Error::NotFound(request.url.clone()))?;
        Ok(FetchResponse::Modified { body, etag: None })
    }
}
