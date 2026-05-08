# Yurt Package Update Flow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the executable `pkg update`, `pkg search`, and `pkg info` cache/query slice described in `docs/superpowers/specs/2026-05-07-yurt-pkg-update-flow-design.md`.

**Architecture:** Add focused repository-cache modules to `yurt-pkg-repo`: fetch traits, POSIX snapshot store, SQLite search index, and update engine. Wire `pkg` to those modules with configurable filesystem roots for tests and image builds. Keep install/archive download/resolver work out of scope.

**Tech Stack:** Rust 2021, `serde`, `serde_json`, `time`, `url`, `sha2`, `fs2` for advisory locks, `rusqlite` for cache queries, deterministic test fetchers/verifiers.

---

## Scope Boundary

This plan implements local repository cache refresh and cache queries. It does **not** implement package archive download, package installation, installed-state DBs, dependency solving, or real Sigstore verification. It uses the existing `BundleVerifier` trait and deterministic test verifier.

The first production-capable fetcher in this plan is filesystem-oriented: `file://` and local directory repository roots. Real HTTP/GitHub authentication remains behind `RepoFetcher` and can be added without changing cache/store/query semantics. This keeps the cache correctness work testable before WASI networking decisions are finalized.

## File Map

- Modify: `Cargo.toml` - add workspace deps `fs2` and `rusqlite`.
- Modify: `crates/yurt-pkg-repo/Cargo.toml` - use `fs2`, `rusqlite`, and define a `test-fixtures` feature for deterministic test fetch/verify helpers.
- Modify: `crates/pkg/Cargo.toml` - depend on `yurt-pkg-repo`, `time`, and `url`; define a `test-fixtures` feature that enables `yurt-pkg-repo/test-fixtures`.
- Create: `crates/yurt-pkg-repo/src/fetch.rs` - `RepoFetcher`, `FetchRequest`, `FetchResponse`, local file fetcher, deterministic memory fetcher for tests.
- Create: `crates/yurt-pkg-repo/src/state.rs` - `SnapshotManifest`, `RepoState`, trust-binding classification, JSON serialization.
- Create: `crates/yurt-pkg-repo/src/store.rs` - repo cache paths, POSIX lock guard, current symlink resolution, staging/snapshot commit, state repair.
- Create: `crates/yurt-pkg-repo/src/search_index.rs` - per-repo SQLite schema plus multi-repo search/info aggregation.
- Create: `crates/yurt-pkg-repo/src/update.rs` - update engine tying trusted repo, fetcher, verifier, store, and search index together.
- Modify: `crates/yurt-pkg-repo/src/verify.rs` - add `NotImplementedVerifier` and gate deterministic verifier helpers behind `test-fixtures`.
- Modify: `crates/yurt-pkg-repo/src/lib.rs` - export new modules/types.
- Create: `crates/yurt-pkg-repo/tests/update_flow.rs` - end-to-end update/store/search tests with memory/file fetchers.
- Modify: `crates/pkg/src/main.rs` - implement `update`, `search`, and `info --repo`; add root override flags hidden from normal help for tests.
- Modify: `crates/pkg/tests/cli.rs` - replace update/search/info stub assertions with executable CLI tests.
- Modify: `docs/pkg.md` - move update/search/info out of deferred behavior and document cache roots/test overrides.

`crates/yurt-pkg-repo/src/cache.rs` remains as the small package-diff helper from the distribution slice. It is not the filesystem cache store; `store.rs` owns snapshot persistence. Do not delete `cache.rs` in this plan.

## Phase 1: Dependencies and Public Module Skeleton

### Task 1: Add Workspace Dependencies

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/yurt-pkg-repo/Cargo.toml`
- Modify: `crates/pkg/Cargo.toml`

- [ ] **Step 1: Add failing dependency check**

Run:

```bash
cargo check -p yurt-pkg-repo
```

Expected: PASS before edits. This records the baseline.

- [ ] **Step 2: Add workspace dependencies**

In root `Cargo.toml`, add under `[workspace.dependencies]`:

```toml
fs2 = "0.4"
rusqlite = { version = "0.32", features = ["bundled"] }
```

In `crates/yurt-pkg-repo/Cargo.toml`, add:

```toml
fs2 = { workspace = true }
rusqlite = { workspace = true }
```

Add to `crates/yurt-pkg-repo/Cargo.toml`:

```toml
[features]
test-fixtures = []
```

In `crates/pkg/Cargo.toml`, add:

```toml
time = { workspace = true }
url = { workspace = true }
yurt-pkg-repo = { path = "../yurt-pkg-repo" }
```

Add to `crates/pkg/Cargo.toml`:

```toml
[features]
test-fixtures = ["yurt-pkg-repo/test-fixtures"]
```

- [ ] **Step 3: Verify dependency graph**

Run:

```bash
cargo check -p yurt-pkg-repo
cargo check -p pkg
```

Expected: PASS.

- [ ] **Step 4: Commit**

Run:

```bash
git add Cargo.toml crates/yurt-pkg-repo/Cargo.toml crates/pkg/Cargo.toml
git commit -m "chore(repo): add update cache dependencies"
```

### Task 2: Add Module Skeletons

**Files:**
- Create: `crates/yurt-pkg-repo/src/fetch.rs`
- Create: `crates/yurt-pkg-repo/src/state.rs`
- Create: `crates/yurt-pkg-repo/src/store.rs`
- Create: `crates/yurt-pkg-repo/src/search_index.rs`
- Create: `crates/yurt-pkg-repo/src/update.rs`
- Modify: `crates/yurt-pkg-repo/src/lib.rs`
- Modify: `crates/yurt-pkg-repo/src/verify.rs`

- [ ] **Step 1: Create empty modules with type exports**

Create the five module files with a one-line module-level comment each:

```rust
//! Repository fetch boundary.
```

Use matching comments for state, store, search index, and update engine.

Update `crates/yurt-pkg-repo/src/lib.rs`:

```rust
pub mod cache;
pub mod fetch;
pub mod metadata;
pub mod search_index;
pub mod select;
pub mod state;
pub mod store;
pub mod update;
pub mod verify;

pub use metadata::{Index, PackageFile, PackageVersion, RepoPackage};
```

- [ ] **Step 2: Add production verifier placeholder and gate test verifier helpers**

In `crates/yurt-pkg-repo/src/verify.rs`, add an always-available production placeholder:

```rust
#[derive(Debug, Clone, Copy, Default)]
pub struct NotImplementedVerifier;

impl BundleVerifier for NotImplementedVerifier {
    fn verify(&self, _input: VerificationInput<'_>) -> Result<VerificationOutput> {
        Err(Error::NotImplemented)
    }
}
```

Move `StaticVerifier` behind:

```rust
#[cfg(any(test, feature = "test-fixtures"))]
```

for both the struct and its `BundleVerifier` impl. This keeps deterministic verifier bypass code out of normal production builds while still allowing integration tests to compile `pkg` with `--features test-fixtures`.

- [ ] **Step 3: Verify module skeleton**

Run:

```bash
cargo check -p yurt-pkg-repo
```

Expected: PASS.

- [ ] **Step 4: Commit**

Run:

```bash
git add crates/yurt-pkg-repo/src
git commit -m "chore(repo): add update flow module skeleton"
```

## Phase 2: Fetch Boundary and State Types

### Task 3: Implement Fetch Boundary

**Files:**
- Modify: `crates/yurt-pkg-repo/src/fetch.rs`
- Test: inline unit tests in `crates/yurt-pkg-repo/src/fetch.rs`

- [ ] **Step 1: Write fetch trait and memory fetcher tests**

Add tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    #[test]
    fn memory_fetcher_returns_modified_and_etag() {
        let mut fetcher = MemoryFetcher::default();
        let url = Url::parse("file:///repo/index.json").unwrap();
        fetcher.insert(url.clone(), b"index".to_vec(), Some("etag-1".to_string()));

        let response = fetcher.fetch(FetchRequest {
            url: &url,
            etag: None,
            credential_origin: None,
        }).unwrap();

        assert_eq!(
            response,
            FetchResponse::Modified {
                body: b"index".to_vec(),
                etag: Some("etag-1".to_string()),
            }
        );
    }

    #[test]
    fn memory_fetcher_returns_not_modified_for_matching_etag() {
        let mut fetcher = MemoryFetcher::default();
        let url = Url::parse("file:///repo/index.json").unwrap();
        fetcher.insert(url.clone(), b"index".to_vec(), Some("etag-1".to_string()));

        let response = fetcher.fetch(FetchRequest {
            url: &url,
            etag: Some("etag-1"),
            credential_origin: None,
        }).unwrap();

        assert_eq!(response, FetchResponse::NotModified);
    }
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p yurt-pkg-repo fetch::tests -- --nocapture
```

Expected: FAIL because `MemoryFetcher` and fetch types do not exist.

- [ ] **Step 3: Implement fetch types**

Implement in `fetch.rs`:

```rust
use std::collections::BTreeMap;

use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum Error {
    #[error("fetch url not found: {0}")]
    NotFound(Url),
    #[error("unsupported fetch scheme for {0}")]
    UnsupportedScheme(Url),
    #[error("failed to read local fetch path {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy)]
pub struct FetchRequest<'a> {
    pub url: &'a Url,
    pub etag: Option<&'a str>,
    pub credential_origin: Option<&'a Url>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchResponse {
    NotModified,
    Modified {
        body: Vec<u8>,
        etag: Option<String>,
    },
}

pub trait RepoFetcher {
    fn fetch(&self, request: FetchRequest<'_>) -> Result<FetchResponse>;
}

#[derive(Debug, Clone, Default)]
pub struct MemoryFetcher {
    entries: BTreeMap<Url, (Vec<u8>, Option<String>)>,
}

impl MemoryFetcher {
    pub fn insert(&mut self, url: Url, body: Vec<u8>, etag: Option<String>) {
        self.entries.insert(url, (body, etag));
    }
}

impl RepoFetcher for MemoryFetcher {
    fn fetch(&self, request: FetchRequest<'_>) -> Result<FetchResponse> {
        // MemoryFetcher intentionally ignores credential_origin; update-flow
        // tests use a recording fetcher to assert origin-scoped auth.
        let Some((body, etag)) = self.entries.get(request.url) else {
            return Err(Error::NotFound(request.url.clone()));
        };
        if request.etag.is_some() && request.etag == etag.as_deref() {
            return Ok(FetchResponse::NotModified);
        }
        Ok(FetchResponse::Modified {
            body: body.clone(),
            etag: etag.clone(),
        })
    }
}
```

- [ ] **Step 4: Add local file fetcher**

Add to `fetch.rs`:

```rust
#[derive(Debug, Clone, Default)]
pub struct LocalFileFetcher;

impl RepoFetcher for LocalFileFetcher {
    fn fetch(&self, request: FetchRequest<'_>) -> Result<FetchResponse> {
        if request.url.scheme() != "file" {
            return Err(Error::UnsupportedScheme(request.url.clone()));
        }
        let path = request
            .url
            .to_file_path()
            .map_err(|_| Error::UnsupportedScheme(request.url.clone()))?;
        let body = std::fs::read(&path).map_err(|source| Error::Io { path, source })?;
        Ok(FetchResponse::Modified { body, etag: None })
    }
}
```

- [ ] **Step 5: Verify tests**

Run:

```bash
cargo test -p yurt-pkg-repo fetch::tests -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
git add crates/yurt-pkg-repo/src/fetch.rs
git commit -m "feat(repo): add repository fetch boundary"
```

### Task 4: Implement Snapshot Manifest and Repo State

**Files:**
- Modify: `crates/yurt-pkg-repo/src/state.rs`
- Test: inline unit tests in `crates/yurt-pkg-repo/src/state.rs`

- [ ] **Step 1: Write state tests**

Add tests with these names and assertions:

```rust
#[test]
fn manifest_serializes_trust_binding_without_priority() {
    // Build manifest from TrustedRepo policy and VerificationOutput exact identity.
    // Assert serialized JSON contains the verified subject/issuer and repo_url.
    // Assert serialized JSON does not contain "priority".
}

#[test]
fn signing_identity_change_resets_security_state() {
    // Manifest stores exact verified identity "actual-subject" / "issuer".
    // TrustedRepo policy changes to "other-subject" / "issuer".
    // Assert SnapshotManifest::trust_change returns TrustChange::SigningIdentity.
}

#[test]
fn url_only_change_keeps_security_state_but_suppresses_fetch_reuse() {
    // Assert SnapshotManifest::trust_change returns TrustChange::UrlOnly
    // when only TrustedRepo.url differs.
}

#[test]
fn priority_only_change_is_not_a_cache_binding_change() {
    // Assert SnapshotManifest::trust_change returns TrustChange::Unchanged
    // when only TrustedRepo.priority differs.
}
```

Use concrete fixtures:

```rust
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;
use yurt_pkg_format::repo::Index;
use yurt_pkg_trust::{SigningIdentity, TrustedRepo};
use crate::verify::VerificationOutput;

let trusted = TrustedRepo {
    id: "official".to_string(),
    url: Url::parse("https://example.com/repo").unwrap(),
    signing: SigningIdentity { subject: "subject".into(), issuer: "issuer".into() },
    priority: 0,
};
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p yurt-pkg-repo state::tests -- --nocapture
```

Expected: FAIL because state types do not exist.

- [ ] **Step 3: Implement state types**

Implement:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotManifest {
    pub schema: u32,
    pub repo_id: String,
    pub repo_url: String,
    pub signing_subject: String,
    pub signing_issuer: String,
    pub index_version: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub integrated_time: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoState {
    pub schema: u32,
    pub repo_id: String,
    pub current_snapshot: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_bundle_etag: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub last_fetched: OffsetDateTime,
    pub consecutive_fetch_failures: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustChange {
    Unchanged,
    UrlOnly,
    SigningIdentity,
}
```

Add methods with these signatures:

```rust
impl SnapshotManifest {
    pub fn from_verified_index(
        repo: &TrustedRepo,
        index: &Index,
        verification: &VerificationOutput,
    ) -> Self;

    pub fn trust_change(&self, repo: &TrustedRepo) -> TrustChange;
}

impl RepoState {
    pub fn without_etags_for_repair(
        repo_id: String,
        current_snapshot: String,
        now: OffsetDateTime,
    ) -> Self;
}
```

- [ ] **Step 4: Verify tests**

Run:

```bash
cargo test -p yurt-pkg-repo state::tests -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/yurt-pkg-repo/src/state.rs
git commit -m "feat(repo): define cache snapshot state"
```

## Phase 3: Store and SQLite Index

### Task 5: Implement POSIX Snapshot Store

**Files:**
- Modify: `crates/yurt-pkg-repo/src/store.rs`
- Test: inline unit tests in `crates/yurt-pkg-repo/src/store.rs`

- [ ] **Step 1: Write store tests**

Add tests with these names and assertions:

```rust
#[test]
fn commit_replaces_current_symlink() {
    // Commit staging snapshot "snap-a", assert read_link("current") == "snapshots/snap-a".
    // Commit staging snapshot "snap-b", assert read_link("current") == "snapshots/snap-b".
    // Assert no current-missing window is observable by running a reader thread
    // that repeatedly read_link("current") while the second commit runs; it may
    // see old or new, but must never see NotFound.
}

#[test]
fn repair_state_clears_stale_etags_on_snapshot_mismatch() {
    // Write state.json with current_snapshot "old" and ETags.
    // Point current at "new"; call repair_state_if_needed.
    // Assert current_snapshot == "new" and both ETag fields are None.
}

#[test]
fn read_lock_and_write_lock_are_available() {
    // Acquire and drop a shared lock, then acquire and drop an exclusive lock
    // against the same repo id.
}

#[test]
fn exclusive_lock_waits_for_shared_lock_to_release() {
    // Hold a shared lock on the repo in one thread.
    // Start a second thread attempting to acquire the exclusive lock.
    // Assert the second thread does not complete until the first lock is dropped.
}
```

Use `tempfile::tempdir()` and assert `std::fs::read_link(repo_dir.join("current"))`.

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p yurt-pkg-repo store::tests -- --nocapture
```

Expected: FAIL because store types do not exist.

- [ ] **Step 3: Implement store paths and locks**

Implement `RepoCacheStore`:

```rust
pub struct RepoCacheStore {
    root: PathBuf,
}

pub enum LockMode {
    Shared,
    Exclusive,
}

pub struct RepoLock {
    file: std::fs::File,
}
```

Methods:

```rust
pub fn new(root: impl Into<PathBuf>) -> Self;
pub fn repo_dir(&self, repo_id: &str) -> PathBuf;
pub fn lock(&self, repo_id: &str, mode: LockMode) -> Result<RepoLock>;
pub fn current_snapshot_id(&self, repo_id: &str) -> Result<Option<String>>;
pub fn read_current_manifest(&self, repo_id: &str) -> Result<Option<SnapshotManifest>>;
pub fn read_state(&self, repo_id: &str) -> Result<Option<RepoState>>;
pub fn write_state(&self, repo_id: &str, state: &RepoState) -> Result<()>;
pub fn repair_state_if_needed(&self, repo_id: &str, now: OffsetDateTime) -> Result<Option<RepoState>>;
pub fn commit_staging(&self, repo_id: &str, staging: &Path, snapshot_id: &str) -> Result<()>;
pub fn snapshot_id(now: OffsetDateTime, index_version: u64, index_bytes: &[u8]) -> String;
```

Implement `commit_staging` through a small private helper:

```rust
fn replace_current_symlink_atomic(repo_dir: &Path, snapshot_id: &str) -> Result<()>;
```

The helper creates `.current.tmp-<snapshot-id>` and then calls `std::fs::rename(tmp, repo_dir.join("current"))`. Do not call `remove_file("current")`, `unlink`, or any unlink-then-symlink sequence.

Use `fs2::FileExt::{lock_shared, lock_exclusive, unlock}` and `std::os::unix::fs::symlink`.
`snapshot_id` must be opaque and collision-resistant enough for crashed retries:

```text
<unix-nanos>-<process-id>-<index-version>-<8-hex-sha256(index-bytes)>
```

`commit_staging` must:

1. rename `staging` to `snapshots/<snapshot-id>`;
2. create a temporary symlink named `.current.tmp-<snapshot-id>` pointing at `snapshots/<snapshot-id>`;
3. atomically `rename` that temporary symlink over `current`;
4. never remove `current` before the replacement symlink exists.

- [ ] **Step 4: Implement same-directory state writes**

`write_state` writes `state.json.tmp-<snapshot-or-random-suffix>` in the repo dir, then `std::fs::rename` to `state.json`. Do not use only the process id as the suffix.

- [ ] **Step 5: Verify store tests**

Run:

```bash
cargo test -p yurt-pkg-repo store::tests -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
git add crates/yurt-pkg-repo/src/store.rs
git commit -m "feat(repo): add posix snapshot cache store"
```

### Task 6: Implement SQLite Search Index

**Files:**
- Modify: `crates/yurt-pkg-repo/src/search_index.rs`
- Test: inline unit tests in `crates/yurt-pkg-repo/src/search_index.rs`

- [ ] **Step 1: Write search index tests**

Add tests with these names and assertions:

```rust
#[test]
fn rebuild_selects_latest_non_yanked_with_build_tiebreak() {
    // Rebuild with 1.0.0/yurt_0, 1.0.0/yurt_1, and yanked 2.0.0/yurt_0.
    // Assert latest_version == Some("1.0.0") and latest_build == Some("yurt_1").
}

#[test]
fn search_groups_by_current_repo_priority() {
    // Create two repo DBs containing "tool".
    // Pass priorities official=10 and overlay=0.
    // Smallest integer priority wins, so overlay must be selected.
    // Assert search returns overlay as the selected repo.
}

#[test]
fn info_returns_versions_newest_first() {
    // Rebuild index with versions 1.0.0 and 1.1.0.
    // Assert info("tool", None) returns 1.1.0 before 1.0.0.
}
```

Fixtures should include `tool` versions `1.0.0/yurt_0`, `1.0.0/yurt_1`, and yanked `2.0.0/yurt_0`, proving `latest_version = 1.0.0` and `latest_build = yurt_1`.

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p yurt-pkg-repo search_index::tests -- --nocapture
```

Expected: FAIL because search index code does not exist.

- [ ] **Step 3: Implement schema and rebuild**

Implement:

```rust
pub struct RepoSearchIndex {
    path: PathBuf,
}

pub struct SearchIndexes {
    repos: Vec<RepoSearchIndex>,
}

pub struct SearchRow {
    pub repo_id: String,
    pub name: String,
    pub latest_version: Option<String>,
    pub latest_build: Option<String>,
    pub latest_yanked: bool,
    pub summary: Option<String>,
}

pub struct InfoResult {
    pub repo_id: String,
    pub package: PackageFile,
}
```

`RepoSearchIndex::rebuild(repo_id, packages)` opens one repo snapshot's SQLite DB, sets `PRAGMA user_version = 1`, creates `packages` and `versions`, and inserts rows.

- [ ] **Step 4: Implement query helpers**

Add:

```rust
impl RepoSearchIndex {
    pub fn search_local(&self, query: &str) -> Result<Vec<SearchRow>>;
    pub fn info_local(&self, name: &str) -> Result<Option<InfoResult>>;
}

impl SearchIndexes {
    pub fn search(&self, query: &str, trusted_priorities: &BTreeMap<String, i64>) -> Result<Vec<SearchRow>>;
    pub fn info(&self, name: &str, repo_filter: Option<&str>, trusted_priorities: &BTreeMap<String, i64>) -> Result<Vec<InfoResult>>;
}
```

Use `RepoSearchIndex` for one snapshot DB. Use `SearchIndexes` for cross-repo grouping and selection with the current priority map. The CLI should build a `SearchIndexes` from every trusted repo's current snapshot.

- [ ] **Step 5: Verify search index tests**

Run:

```bash
cargo test -p yurt-pkg-repo search_index::tests -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
git add crates/yurt-pkg-repo/src/search_index.rs
git commit -m "feat(repo): add sqlite search index"
```

## Phase 4: Update Engine

### Task 7: Implement Update Engine Happy Path

**Files:**
- Modify: `crates/yurt-pkg-repo/src/update.rs`
- Test: `crates/yurt-pkg-repo/tests/update_flow.rs`

- [ ] **Step 1: Write end-to-end happy path test**

Create `tests/update_flow.rs` with a test `update_fetches_signed_index_and_rebuilds_search_index`.

The test should:
- create `TrustedRepo` for `file:///repo/`;
- create in-memory index bytes, bundle bytes, and `packages/tool.json`;
- use `MemoryFetcher`;
- use `StaticVerifier` with matching subject/issuer;
- call `UpdateEngine::update_repo`;
- assert `current` points at a snapshot;
- open snapshot `db.sqlite` through `RepoSearchIndex` and find `tool`.

Also add a test `relative_package_url_resolves_against_repo_base`:

```rust
#[test]
fn relative_package_url_resolves_against_repo_base() {
    // Index entry for "tool" has url = "pkg/tool-v1.json".
    // Seed MemoryFetcher at file:///repo/pkg/tool-v1.json, not packages/tool.json.
    // Run update and assert it succeeds and the committed snapshot persists
    // packages/tool.json using the index map key as the local filename.
}
```

- [ ] **Step 2: Run test and verify failure**

Run:

```bash
cargo test -p yurt-pkg-repo --features test-fixtures --test update_flow update_fetches_signed_index_and_rebuilds_search_index -- --nocapture
```

Expected: FAIL because `UpdateEngine` does not exist.

- [ ] **Step 3: Implement update engine types**

In `update.rs` implement:

```rust
pub struct UpdateOptions {
    pub now: OffsetDateTime,
    pub freshness: Freshness,
}

pub struct UpdateEngine<F, V> {
    pub fetcher: F,
    pub verifier: V,
    pub trust_root: TrustRoot,
    pub cache_store: RepoCacheStore,
}

pub struct RepoUpdateOutcome {
    pub repo_id: String,
    pub changed: bool,
    pub index_version: u64,
    pub warnings: Vec<String>,
}
```

- [ ] **Step 4: Implement modified-index flow**

Implement steps 1-19 from the spec for modified index responses. Use:

```rust
sha2::Sha256::digest(&package_bytes)
```

to verify package JSON hashes, and compare byte length to `RepoPackage.size`.

Resolve every package file from the signed `RepoPackage.url` field, not from a hardcoded `packages/<name>.json` path:
- relative URLs resolve against `TrustedRepo.url`;
- absolute URLs are used as-is after validation;
- local persistence always writes `packages/<index-map-key>.json`.

Set `FetchRequest.credential_origin` to the trusted repo origin for same-origin package URLs. Set it to `None` for absolute package URLs on a different origin so repository credentials cannot leak cross-origin.

- [ ] **Step 5: Verify happy path**

Run:

```bash
cargo test -p yurt-pkg-repo --features test-fixtures --test update_flow update_fetches_signed_index_and_rebuilds_search_index -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
git add crates/yurt-pkg-repo/src/update.rs crates/yurt-pkg-repo/tests/update_flow.rs
git commit -m "feat(repo): implement repository update happy path"
```

### Task 8: Implement Update Edge Cases

**Files:**
- Modify: `crates/yurt-pkg-repo/src/update.rs`
- Modify: `crates/yurt-pkg-repo/tests/update_flow.rs`

- [ ] **Step 1: Add edge-case tests**

Add tests with these names and assertions:

```rust
#[test]
fn not_modified_revalidates_cached_expiry_and_updates_last_fetched() {
    // Seed a current snapshot and state ETag.
    // Return FetchResponse::NotModified for index.
    // Assert current snapshot is unchanged and state.last_fetched advances.
}

#[test]
fn rollback_uses_current_manifest_index_version() {
    // Seed current manifest index_version 5.
    // Serve signed index_version 4.
    // Assert update fails with rollback and current symlink is unchanged.
    // Serve signed index_version 5.
    // Assert equal version also fails because new versions must be strictly greater.
}

#[test]
fn rekor_time_uses_current_manifest_integrated_time() {
    // Seed current manifest integrated_time T2.
    // Static verifier returns integrated_time T1 < T2.
    // Assert update fails and current symlink is unchanged.
}

#[test]
fn package_name_must_match_index_key() {
    // Index map key is "tool"; fetched package JSON has name "other".
    // Assert update fails before commit.
}

#[test]
fn url_only_change_keeps_rollback_protection() {
    // Seed current manifest signed by subject/issuer at old URL index_version 5.
    // TrustedRepo has same subject/issuer at new URL.
    // Serve index_version 4 and assert rollback is still rejected.
}

#[test]
fn signing_identity_change_resets_rollback_protection() {
    // Seed current manifest signed by old subject at index_version 5.
    // TrustedRepo uses new subject and verifier returns new subject.
    // Serve index_version 1 and assert update succeeds.
}

#[test]
fn failed_update_preserves_previous_current_snapshot() {
    // Seed current snapshot.
    // Serve package JSON with wrong hash.
    // Assert current symlink still points to the original snapshot.
}

#[test]
fn current_snapshot_repair_clears_stale_etags() {
    // Seed current symlink to "new" and state.current_snapshot = "old" with ETags.
    // Run update; assert state current_snapshot is "new" and ETags are None.
}

#[test]
fn absolute_package_url_does_not_inherit_cross_origin_credentials() {
    // TrustedRepo.url is https://repo.example/index/.
    // Index entry for "tool" uses absolute URL https://cdn.example/tool.json.
    // Use a RecordingFetcher that captures each FetchRequest.
    // Assert the package fetch URL is https://cdn.example/tool.json and
    // request.credential_origin is None.
}

#[test]
fn unchanged_package_files_are_carried_forward_to_new_snapshot() {
    // Seed current snapshot with packages/tool.json and index hash H.
    // Serve a new index_version where "tool" has the same hash/size/url and
    // "new-tool" is added.
    // Assert the new snapshot contains both packages/tool.json and
    // packages/new-tool.json even though only new-tool was fetched.
}

#[test]
fn changed_package_file_replaces_previous_snapshot_copy() {
    // Seed current snapshot with packages/tool.json hash H1.
    // Serve a new index where "tool" has hash H2 and package JSON version 1.1.0.
    // Assert the new snapshot packages/tool.json has the H2 bytes and the
    // search db reports version 1.1.0.
}

#[test]
fn removed_package_files_are_omitted_from_new_snapshot() {
    // Seed current snapshot with packages/tool.json and packages/old.json.
    // Serve a new index containing only "tool".
    // Assert the committed snapshot contains packages/tool.json and does not
    // contain packages/old.json.
}

#[test]
fn failed_fetch_increments_failure_count_and_success_resets_it() {
    // Seed state.consecutive_fetch_failures = 0.
    // Make index fetch fail after the fetch phase begins.
    // Assert the returned error is the fetch error and state failure count is 1.
    // Then serve a valid update and assert state failure count resets to 0.
}

#[test]
fn retried_same_index_version_after_abandoned_staging_does_not_collide() {
    // Leave an orphan snapshots/<opaque-id-for-version-6> directory that is not current.
    // Serve index_version 6.
    // Assert update commits a different snapshot id and succeeds.
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p yurt-pkg-repo --features test-fixtures --test update_flow -- --nocapture
```

Expected: FAIL on newly added tests.

- [ ] **Step 3: Implement 304 path**

When index fetch is `NotModified`, skip bundle fetch, validate current index freshness, update `state.json.last_fetched`, and preserve `current`.

- [ ] **Step 4: Implement rollback/Rekor/trust-change rules**

Use `TrustChange` from `state.rs`:
- `SigningIdentity`: no cached ETags, no package hash reuse, no rollback/Rekor baseline.
- `UrlOnly`: no cached ETags, no package hash reuse, keep rollback/Rekor baseline.
- `Unchanged`: use cached ETags and package hashes.

- [ ] **Step 5: Implement failure counter**

On failures after fetch phase begins and before commit, increment `consecutive_fetch_failures` under lock and return the original error.

- [ ] **Step 6: Implement complete-snapshot package persistence**

When a modified index is accepted, the new staging directory must become a complete snapshot:
- fetch changed packages from resolved `RepoPackage.url`;
- copy unchanged package JSON files from the current snapshot when hash and size match;
- omit package JSON files whose index keys are absent from the new index;
- verify every persisted package file's `PackageFile.name` equals the signed index map key.

- [ ] **Step 7: Verify edge cases**

Run:

```bash
cargo test -p yurt-pkg-repo --features test-fixtures --test update_flow -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Commit**

Run:

```bash
git add crates/yurt-pkg-repo/src/update.rs crates/yurt-pkg-repo/tests/update_flow.rs
git commit -m "feat(repo): enforce update cache safety rules"
```

## Phase 5: CLI Integration

### Task 9: Wire `pkg update`

**Files:**
- Modify: `crates/pkg/src/main.rs`
- Modify: `crates/pkg/tests/cli.rs`

- [ ] **Step 1: Add CLI test for local repo update**

In `crates/pkg/tests/cli.rs`, add a test that creates temporary:

```text
etc/yurt-pkg/trusted-repos.toml
etc/yurt-pkg/sigstore-trust-root/fulcio-root.pem
etc/yurt-pkg/sigstore-trust-root/rekor.pub
var/cache/yurt-pkg/repos/
repo/index.json
repo/index.json.bundle
repo/packages/tool.json
```

Run:

```rust
cmd.args([
    "--etc-root", etc.path().to_str().unwrap(),
    "--cache-root", cache.path().to_str().unwrap(),
    "update",
]);
```

Expected stdout contains `updated official`.

Also add:

```rust
#[test]
fn cli_update_reports_consecutive_failure_count() {
    // Configure a trusted local repo whose index.json is missing.
    // Run pkg update twice with the same cache root.
    // Assert the second stderr mentions 2 consecutive update failures for official.
}
```

- [ ] **Step 2: Run test and verify failure**

Run:

```bash
cargo test -p pkg --features test-fixtures --test cli cli_update -- --nocapture
```

Expected: FAIL because root flags and update implementation do not exist.

- [ ] **Step 3: Add hidden root flags**

Change `Cli`:

```rust
struct Cli {
    #[arg(long, hide = true, default_value = "/etc")]
    etc_root: PathBuf,
    #[arg(long, hide = true, default_value = "/var/cache/yurt-pkg/repos")]
    cache_root: PathBuf,
    #[command(subcommand)]
    command: Command,
}
```

- [ ] **Step 4: Implement update command**

Read:

```text
<etc_root>/yurt-pkg/trusted-repos.toml
<etc_root>/yurt-pkg/sigstore-trust-root/
```

Default production wiring must use:

```rust
use yurt_pkg_repo::verify::NotImplementedVerifier;
```

and return the existing `bundle verification is not wired to sigstore yet` error until the real Sigstore verifier lands. Do not add an environment variable or hidden runtime flag that selects a bypass verifier.

For integration tests only, compile `pkg` with `--features test-fixtures` and use a compile-time branch:

```rust
#[cfg(feature = "test-fixtures")]
let verifier = yurt_pkg_repo::verify::StaticVerifier {
    output: yurt_pkg_repo::verify::VerificationOutput {
        integrated_time: now,
        subject: repo.signing.subject.clone(),
        issuer: repo.signing.issuer.clone(),
    },
};

#[cfg(not(feature = "test-fixtures"))]
let verifier = yurt_pkg_repo::verify::NotImplementedVerifier;
```

The test fixture feature is acceptable because the bypass branch is compiled out of default production binaries.

- [ ] **Step 5: Verify update CLI**

Run:

```bash
cargo test -p pkg --features test-fixtures --test cli cli_update -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
git add crates/pkg/src/main.rs crates/pkg/tests/cli.rs
git commit -m "feat(pkg): wire repository update command"
```

### Task 10: Wire `pkg search` and `pkg info`

**Files:**
- Modify: `crates/pkg/src/main.rs`
- Modify: `crates/pkg/tests/cli.rs`
- Modify: `docs/pkg.md`

- [ ] **Step 1: Add CLI tests**

Add tests with these names and assertions:

```rust
#[test]
fn cli_search_reads_cache_without_network() {
    // Prepare a cache with a committed snapshot and db.sqlite.
    // Run pkg search tool with --cache-root and --etc-root.
    // Assert stdout contains "tool" and no fetcher environment variables are required.
}

#[test]
fn cli_info_lists_versions_and_dependencies() {
    // Prepare a cache containing tool -> depends libc ^0.1.
    // Run pkg info tool.
    // Assert stdout contains version, signing identity, and dependency.
}

#[test]
fn cli_info_repo_filters_repo() {
    // Prepare two repo caches containing tool.
    // Run pkg info tool --repo overlay.
    // Assert stdout contains "repo: overlay" and does not render the official repo entry.
}

#[test]
fn cli_search_no_cache_exits_nonzero_and_suggests_update() {
    // Configure trusted-repos.toml but do not create a current snapshot.
    // Run pkg search tool.
    // Assert nonzero exit and stderr contains "run pkg update".
}

#[test]
fn cli_search_warns_on_stale_cache_and_failure_count() {
    // Prepare a cache whose manifest expires_at is past freshness grace and
    // whose state.consecutive_fetch_failures is 2.
    // Run pkg search tool.
    // Assert stdout still includes cached rows, stderr warns that the cache is
    // stale, and stderr warns about 2 consecutive update failures.
}

#[test]
fn cli_info_warns_on_stale_cache_and_failure_count() {
    // Same fixture as search, but run pkg info tool.
    // Assert cached info renders and stderr includes both warnings.
}

#[test]
fn cli_search_refuses_signing_identity_change() {
    // Cache manifest has subject "old"; trusted-repos.toml has subject "new".
    // Run pkg search tool.
    // Assert nonzero exit and stderr says trusted config for repo official changed; run pkg update.
}

#[test]
fn cli_info_refuses_signing_identity_change() {
    // Cache manifest has subject "old"; trusted-repos.toml has subject "new".
    // Run pkg info tool.
    // Assert nonzero exit and stderr says trusted config for repo official changed; run pkg update.
}

#[test]
fn cli_info_warns_on_url_only_change() {
    // Cache manifest and trusted config have the same subject/issuer but different repo_url.
    // Run pkg info tool.
    // Assert info renders and stderr warns that repo official URL changed and pkg update should refresh it.
}
```

Use the cache produced by the update test fixture.

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p pkg --test cli cli_search -- --nocapture
cargo test -p pkg --test cli cli_info -- --nocapture
```

Expected: FAIL because search/info still bail.

- [ ] **Step 3: Add `--repo` to info command**

Change:

```rust
Info {
    name: String,
    #[arg(long)]
    repo: Option<String>,
}
```

- [ ] **Step 4: Implement search/info**

For each trusted repo:
- take shared lock;
- resolve current snapshot;
- validate signing identity against current trusted config;
- warn on URL-only change;
- open `db.sqlite`;
- query via `RepoSearchIndex`;
- aggregate selected rows with `SearchIndexes` and current trusted priorities.

Render simple text:

```text
tool 1.0.0-yurt_0 official
```

For info:

```text
tool
repo: official
version: 1.0.0-yurt_0
url: https://example.com/tool.yurtpkg
sha256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
signing: subject / issuer
depends:
  libc ^0.1
```

- [ ] **Step 5: Verify CLI tests**

Run:

```bash
cargo test -p pkg --test cli cli_search -- --nocapture
cargo test -p pkg --test cli cli_info -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Update docs**

In `docs/pkg.md`, change update/search/info from deferred to implemented:

```markdown
`pkg update` refreshes trusted repository metadata into `/var/cache/yurt-pkg/repos/<repo-id>/`.
`pkg search` and `pkg info` read only the local cache.
If the cache is stale or the previous update failed, search/info print warnings while still rendering usable cached metadata.
If no cache exists, search/info exit nonzero and suggest running `pkg update`.
```

Leave install/upgrade/remove/list in deferred behavior.

- [ ] **Step 7: Full verification**

Run:

```bash
cargo test --workspace
cargo test -p yurt-pkg-repo --features test-fixtures --test update_flow -- --nocapture
cargo test -p pkg --features test-fixtures --test cli -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Commit**

Run:

```bash
git add crates/pkg/src/main.rs crates/pkg/tests/cli.rs docs/pkg.md
git commit -m "feat(pkg): implement search and info cache queries"
```

## Self-Review Checklist

- [ ] Every spec requirement maps to a task:
  - locking and POSIX symlink snapshot commit: Task 5;
  - manifest/state split and repair: Tasks 4-5;
  - update modified/304 flows: Tasks 7-8;
  - rollback/Rekor/trust-change rules: Tasks 4 and 8;
  - package URL resolution and name/hash/size checks: Tasks 7-8;
  - changed/unchanged/removed package snapshot persistence: Task 8;
  - SQLite per-repo indexes and cross-repo search/info aggregation: Task 6;
  - CLI update/search/info, stale/missing-cache warnings, and trust-change display behavior: Tasks 9-10.
- [ ] No install/archive download/resolver behavior is implemented.
- [ ] No real HTTP/GitHub auth is required in this plan; credential-origin handling is asserted at the fetch boundary for future auth wiring.
- [ ] All commands in task steps are exact and scoped.
- [ ] `cargo test --workspace` passes at the end.
