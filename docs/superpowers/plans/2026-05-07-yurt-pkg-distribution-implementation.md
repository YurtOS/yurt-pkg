# Yurt Package Distribution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the package repository metadata, trust metadata, recipe linting, and initial `pkg` client command foundation described in `docs/superpowers/specs/2026-05-07-yurt-pkg-distribution-design.md`.

**Architecture:** Land this as vertical, testable slices. First update the archive schema and builder input, then add trust/repository crates, then add the `pkg` command shell and repository CI checks. Sigstore verification is introduced behind a narrow verifier trait so repository parsing and client behavior can be tested with deterministic fixtures before the WASI Sigstore smoke test decides whether to use the upstream crate or the fallback verifier.

**Tech Stack:** Rust 2021 workspace, `serde`, `serde_json`, `toml`, `semver`, `sha2`, `clap`, `thiserror`, `tempfile`, `time`, `url`, Sigstore Bundle verification via `sigstore` crate if the smoke test passes.

---

## Scope Split

This spec spans five subsystems: package format migration, trust policy, repository metadata/cache, in-sandbox `pkg`, and repository publishing CI. Implement them in this order. Do not start resolver internals, install hooks, file ownership conflict handling, or true revocation lists in this plan; the spec explicitly defers them.

The only resolver-related behavior in this plan is metadata shape and command scaffolding. `pkg install` and `pkg upgrade` should parse arguments and return a clear "planner not implemented in this slice" error until the resolver/installer spec exists.

Executable `pkg update` is also deferred to a follow-up update-flow plan. This plan builds the metadata, freshness, rollback, trust-root, and verifier boundaries that `pkg update` needs, but it does not implement HTTP fetching, 304 handling, `meta.json` persistence, `db.sqlite`, or package-file cache writes.

`pkg trust` is not implemented as a wildcard parser in this plan because the distribution spec defers its concrete subcommand surface. Add it when the trust-management UX is specified.

## File Map

- Modify: `Cargo.toml` - add shared dependencies first, then add new workspace members only when their crate directories are created.
- Modify: `crates/yurt-pkg-format/src/manifest.rs` - migrate dependency schema from transparent strings to `{ name, req }`.
- Modify: `crates/yurt-pack/src/manifest_toml.rs` - parse dependency maps in builder manifests.
- Modify: `crates/yurt-pack/src/main.rs` - emit new dependency shape and canonical `.yurtpkg` artifact basename.
- Modify: `crates/yurt-pack/tests/build.rs` - cover dependency schema and artifact rename.
- Create: `crates/yurt-pkg-trust/Cargo.toml`
- Create: `crates/yurt-pkg-trust/src/lib.rs` - trusted repo TOML parsing, subject/issuer policy, repo id validation.
- Create: `crates/yurt-pkg-trust/tests/trusted_repos.rs`
- Create: `crates/yurt-pkg-repo/Cargo.toml`
- Create: `crates/yurt-pkg-repo/src/lib.rs`
- Create: `crates/yurt-pkg-repo/src/metadata.rs` - `index.json`, `packages/<name>.json`, freshness, rollback validation.
- Create: `crates/yurt-pkg-repo/src/cache.rs` - package diffing only; executable HTTP/cache persistence is deferred.
- Create: `crates/yurt-pkg-repo/src/select.rs` - multi-repo selection by priority for search/info display.
- Create: `crates/yurt-pkg-repo/src/verify.rs` - verifier trait plus deterministic test verifier.
- Create: `crates/yurt-pkg-repo/tests/metadata.rs`
- Create: `crates/yurt-pkg-repo/tests/cache.rs`
- Create: `crates/pkg/Cargo.toml`
- Create: `crates/pkg/src/main.rs` - command surface.
- Create: `crates/pkg/tests/cli.rs`
- Create: `crates/yurt-repo-ci/Cargo.toml`
- Create: `crates/yurt-repo-ci/src/main.rs` - recipe lint and signer-continuity CLI.
- Create: `crates/yurt-repo-ci/tests/recipe_lint.rs`
- Create: `docs/superpowers/specs/2026-05-07-yurt-pkg-resolver-installer-design.md` - short stub that records the deferred resolver/installer boundary.
- Create: `docs/superpowers/specs/2026-05-07-yurt-pkg-update-flow-design.md` - short stub that records the deferred executable update/cache boundary.

## Phase 1: Format and Builder Schema

### Task 1: Add Shared Dependencies and Baseline Tag

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add shared dependencies only**

Do not add new workspace members in this task. Cargo refuses to load workspace members whose directories do not exist, and Tasks 2-3 still need to run cargo commands before the new crates are created.

Add these shared dependencies under `[workspace.dependencies]` in `Cargo.toml`:

```toml
semver = { version = "1", features = ["serde"] }
toml = "0.8"
time = { version = "0.3", features = ["formatting", "parsing", "serde", "serde-well-known", "macros"] }
url = { version = "2", features = ["serde"] }
assert_cmd = "2"
predicates = "3"
```

- [ ] **Step 2: Tag the pre-plan baseline**

Run:

```bash
git tag yurt-pkg-distribution-plan-base HEAD
```

Expected: tag created at the current pre-implementation commit. If this fails because the tag already exists, stop and inspect the previous run before continuing.

- [ ] **Step 3: Run metadata check**

Run: `cargo metadata --no-deps`

Expected: PASS. If this fails with "failed to load manifest for workspace member", remove the non-existent member from `Cargo.toml`; new members are added in the task that creates each crate.

### Task 2: Migrate Package Dependencies to `{ name, req }`

**Files:**
- Modify: `crates/yurt-pkg-format/src/manifest.rs`
- Test: inline unit tests in `crates/yurt-pkg-format/src/manifest.rs`

- [ ] **Step 1: Write failing tests for dependency validation**

Add this test module to the bottom of `crates/yurt-pkg-format/src/manifest.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_dependencies_use_schema_version_2() {
        assert_eq!(SCHEMA_VERSION, 2);
    }

    #[test]
    fn dependency_requires_valid_name_and_semver_req() {
        let dep = Depends {
            name: "libfoo".to_string(),
            req: "^1.2".to_string(),
        };
        dep.validate().unwrap();

        let bad_name = Depends {
            name: "LibFoo".to_string(),
            req: "^1.2".to_string(),
        };
        assert!(bad_name.validate().is_err());

        let bad_req = Depends {
            name: "libfoo".to_string(),
            req: "=>1".to_string(),
        };
        assert!(bad_req.validate().is_err());
    }

    #[test]
    fn dependency_serializes_as_name_req_object() {
        let dep = Depends {
            name: "libfoo".to_string(),
            req: "^1.2".to_string(),
        };
        let json = serde_json::to_string(&dep).unwrap();
        assert_eq!(json, r#"{"name":"libfoo","req":"^1.2"}"#);
    }
}
```

- [ ] **Step 2: Run the targeted tests and verify failure**

Run: `cargo test -p yurt-pkg-format manifest::tests::dependency_ -- --nocapture`

Expected: FAIL because `SCHEMA_VERSION` is still 1 and `Depends` is still a transparent tuple string.

- [ ] **Step 3: Bump package manifest schema version**

Change `SCHEMA_VERSION` in `crates/yurt-pkg-format/src/manifest.rs`:

```rust
/// Schema version for `info/index.json`.
///
/// Version 2 changes `depends` from the v1 transparent string list to
/// structured `{ name, req }` objects. No released v1 archives with
/// non-empty dependencies exist in the wild, so no compatibility reader is
/// required for this migration.
pub const SCHEMA_VERSION: u32 = 2;
```

- [ ] **Step 4: Implement the dependency struct**

Replace the current `Depends` definition and impl with:

```rust
/// A dependency constraint in package metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Depends {
    pub name: String,
    pub req: String,
}

impl Depends {
    pub fn validate(&self) -> Result<()> {
        validate_package_name(&self.name)?;
        semver::VersionReq::parse(&self.req).map_err(|err| {
            Error::InvalidManifest(format!(
                "invalid dependency requirement '{}' for '{}': {err}",
                self.req, self.name
            ))
        })?;
        Ok(())
    }

    pub fn name(&self) -> &str {
        self.name.as_str()
    }
}
```

Add `semver = { workspace = true }` to `crates/yurt-pkg-format/Cargo.toml`.

- [ ] **Step 5: Run tests**

Run: `cargo test -p yurt-pkg-format`

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
git add Cargo.toml crates/yurt-pkg-format/Cargo.toml crates/yurt-pkg-format/src/manifest.rs
git commit -m "feat(format): bump schema for structured dependencies"
```

### Task 3: Update `yurt-pack` Manifest Parsing and Artifact Name

**Files:**
- Modify: `crates/yurt-pack/src/manifest_toml.rs`
- Modify: `crates/yurt-pack/src/main.rs`
- Modify: `crates/yurt-pack/tests/build.rs`

- [ ] **Step 1: Write failing integration test**

In `crates/yurt-pack/tests/build.rs`, update the manifest text in `builds_an_archive_from_a_staged_tree` so dependencies are a TOML table:

```toml
[depends]
libfoo = "^1.2"
libbar = ">=0.5, <1.0"
```

This root-level `[depends]` table is intentionally for the local `yurt-pack.toml` builder input. Repository recipes use `[package.depends]`; the two TOML shapes are different because recipes wrap package fields under `[package]`, while `yurt-pack` preserves its existing flat manifest style.

Update the artifact assertion:

```rust
let artifact = out.join("demo-0.1.0-yurt_0.yurtpkg");
```

After reading the archive, assert structured dependencies:

```rust
assert_eq!(r.index.depends.len(), 2);
assert_eq!(r.index.depends[0].name, "libbar");
assert_eq!(r.index.depends[0].req, ">=0.5, <1.0");
assert_eq!(r.index.depends[1].name, "libfoo");
assert_eq!(r.index.depends[1].req, "^1.2");
```

Use sorted output from the implementation so this assertion is stable.

- [ ] **Step 2: Run test and verify failure**

Run: `cargo test -p yurt-pack builds_an_archive_from_a_staged_tree -- --nocapture`

Expected: FAIL because `depends` is still parsed as `Vec<String>` and artifact basename still ends in `.tar.zst`.

- [ ] **Step 3: Change TOML schema**

In `crates/yurt-pack/src/manifest_toml.rs`, add:

```rust
use std::collections::BTreeMap;
```

Change the `depends` field:

```rust
#[serde(default)]
pub depends: BTreeMap<String, String>,
```

- [ ] **Step 4: Emit structured dependencies**

In `crates/yurt-pack/src/main.rs`, replace dependency mapping with:

```rust
depends: manifest
    .depends
    .iter()
    .map(|(name, req)| Depends {
        name: name.clone(),
        req: req.clone(),
    })
    .collect(),
```

- [ ] **Step 5: Rename artifact basename**

In `crates/yurt-pkg-format/src/manifest.rs`, change `artifact_basename` to:

```rust
/// Canonical published artifact basename: `<name>-<version>-<build>.yurtpkg`.
pub fn artifact_basename(&self) -> String {
    format!("{}-{}-{}.yurtpkg", self.name, self.version, self.build)
}
```

- [ ] **Step 6: Run builder tests**

Run: `cargo test -p yurt-pack`

Expected: PASS.

- [ ] **Step 7: Commit**

Run:

```bash
git add crates/yurt-pack/src/manifest_toml.rs crates/yurt-pack/src/main.rs crates/yurt-pack/tests/build.rs crates/yurt-pkg-format/src/manifest.rs
git commit -m "feat(pack): emit distribution package metadata"
```

## Phase 2: Trust Policy Crate

### Task 4: Create `yurt-pkg-trust`

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/yurt-pkg-trust/Cargo.toml`
- Create: `crates/yurt-pkg-trust/src/lib.rs`
- Create: `crates/yurt-pkg-trust/tests/trusted_repos.rs`

- [ ] **Step 1: Add workspace member and create crate manifest**

Add `"crates/yurt-pkg-trust"` to the root `Cargo.toml` workspace `members` list.

Create `crates/yurt-pkg-trust/Cargo.toml`:

```toml
[package]
name = "yurt-pkg-trust"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
serde = { workspace = true }
thiserror = { workspace = true }
toml = { workspace = true }
url = { workspace = true }
yurt-pkg-format = { path = "../yurt-pkg-format" }

[dev-dependencies]
tempfile = { workspace = true }
```

- [ ] **Step 2: Write tests**

Create `crates/yurt-pkg-trust/tests/trusted_repos.rs`:

```rust
use std::path::PathBuf;

use yurt_pkg_trust::{SigningIdentity, TrustRoot, TrustedRepos};

#[test]
fn parses_trusted_repos_with_subject_and_issuer() {
    let text = r#"
[[repo]]
id = "yurt-core"
url = "https://github.com/YurtOS/yurt-packages"
signing_subject = "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main"
signing_issuer = "https://token.actions.githubusercontent.com"
priority = 0
"#;

    let repos = TrustedRepos::from_toml_str(text).unwrap();
    let repo = repos.get("yurt-core").unwrap();
    assert_eq!(repo.priority, 0);
    assert_eq!(
        repo.signing,
        SigningIdentity {
            subject: "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main".to_string(),
            issuer: "https://token.actions.githubusercontent.com".to_string(),
        }
    );
}

#[test]
fn rejects_missing_issuer_and_duplicate_ids() {
    let missing_issuer = r#"
[[repo]]
id = "yurt-core"
url = "https://github.com/YurtOS/yurt-packages"
signing_subject = "subject"
priority = 0
"#;
    assert!(TrustedRepos::from_toml_str(missing_issuer).is_err());

    let duplicate = r#"
[[repo]]
id = "core"
url = "https://example.com/one"
signing_subject = "subject"
signing_issuer = "issuer"
priority = 0

[[repo]]
id = "core"
url = "https://example.com/two"
signing_subject = "subject"
signing_issuer = "issuer"
priority = 1
"#;
    assert!(TrustedRepos::from_toml_str(duplicate).is_err());
}

#[test]
fn signing_identity_matches_both_fields() {
    let expected = SigningIdentity {
        subject: "subject".to_string(),
        issuer: "issuer".to_string(),
    };
    assert!(expected.matches("subject", "issuer"));
    assert!(!expected.matches("subject", "other"));
    assert!(!expected.matches("other", "issuer"));
}

#[test]
fn trust_root_records_fulcio_and_rekor_paths() {
    let root = TrustRoot::from_dir(PathBuf::from("/etc/yurt-pkg/sigstore-trust-root"));
    assert_eq!(
        root.fulcio_root_pem,
        PathBuf::from("/etc/yurt-pkg/sigstore-trust-root/fulcio-root.pem")
    );
    assert_eq!(
        root.rekor_public_key,
        PathBuf::from("/etc/yurt-pkg/sigstore-trust-root/rekor.pub")
    );
}
```

- [ ] **Step 3: Run tests and verify failure**

Run: `cargo test -p yurt-pkg-trust`

Expected: FAIL because the crate has no implementation.

- [ ] **Step 4: Implement crate**

Create `crates/yurt-pkg-trust/src/lib.rs`:

```rust
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;
use yurt_pkg_format::validate_package_name;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to parse trusted repos TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("invalid repo id '{0}'")]
    InvalidRepoId(String),
    #[error("duplicate repo id '{0}'")]
    DuplicateRepoId(String),
    #[error("invalid repo url for '{id}': {source}")]
    InvalidRepoUrl { id: String, source: url::ParseError },
    #[error("repo '{0}' has an empty signing subject")]
    EmptySubject(String),
    #[error("repo '{0}' has an empty signing issuer")]
    EmptyIssuer(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SigningIdentity {
    pub subject: String,
    pub issuer: String,
}

impl SigningIdentity {
    pub fn matches(&self, subject: &str, issuer: &str) -> bool {
        self.subject == subject && self.issuer == issuer
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustRoot {
    pub fulcio_root_pem: std::path::PathBuf,
    pub rekor_public_key: std::path::PathBuf,
}

impl TrustRoot {
    pub fn from_dir(dir: impl Into<std::path::PathBuf>) -> Self {
        let dir = dir.into();
        Self {
            fulcio_root_pem: dir.join("fulcio-root.pem"),
            rekor_public_key: dir.join("rekor.pub"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedRepo {
    pub id: String,
    pub url: Url,
    pub signing: SigningIdentity,
    pub priority: i64,
}

#[derive(Debug, Clone, Default)]
pub struct TrustedRepos {
    repos: BTreeMap<String, TrustedRepo>,
}

impl TrustedRepos {
    pub fn from_toml_str(text: &str) -> Result<Self> {
        let raw: RawTrustedRepos = toml::from_str(text)?;
        let mut seen = BTreeSet::new();
        let mut repos = BTreeMap::new();
        for repo in raw.repo {
            validate_package_name(&repo.id).map_err(|_| Error::InvalidRepoId(repo.id.clone()))?;
            if !seen.insert(repo.id.clone()) {
                return Err(Error::DuplicateRepoId(repo.id));
            }
            if repo.signing_subject.trim().is_empty() {
                return Err(Error::EmptySubject(repo.id));
            }
            if repo.signing_issuer.trim().is_empty() {
                return Err(Error::EmptyIssuer(repo.id));
            }
            let url = Url::parse(&repo.url).map_err(|source| Error::InvalidRepoUrl {
                id: repo.id.clone(),
                source,
            })?;
            let trusted = TrustedRepo {
                id: repo.id.clone(),
                url,
                signing: SigningIdentity {
                    subject: repo.signing_subject,
                    issuer: repo.signing_issuer,
                },
                priority: repo.priority.unwrap_or(0),
            };
            repos.insert(repo.id, trusted);
        }
        Ok(Self { repos })
    }

    pub fn get(&self, id: &str) -> Option<&TrustedRepo> {
        self.repos.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &TrustedRepo> {
        self.repos.values()
    }
}

#[derive(Debug, Deserialize)]
struct RawTrustedRepos {
    #[serde(default)]
    repo: Vec<RawTrustedRepo>,
}

#[derive(Debug, Deserialize)]
struct RawTrustedRepo {
    id: String,
    url: String,
    signing_subject: String,
    signing_issuer: String,
    priority: Option<i64>,
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p yurt-pkg-trust`

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
git add Cargo.toml crates/yurt-pkg-trust
git commit -m "feat(trust): parse trusted repository policy"
```

## Phase 3: Repository Metadata and Cache

### Task 5: Create Repository Metadata Types

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/yurt-pkg-repo/Cargo.toml`
- Create: `crates/yurt-pkg-repo/src/lib.rs`
- Create: `crates/yurt-pkg-repo/src/metadata.rs`
- Create: `crates/yurt-pkg-repo/tests/metadata.rs`

- [ ] **Step 1: Add workspace member and create crate manifest**

Add `"crates/yurt-pkg-repo"` to the root `Cargo.toml` workspace `members` list.

Create `crates/yurt-pkg-repo/Cargo.toml`:

```toml
[package]
name = "yurt-pkg-repo"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
hex = { workspace = true }
semver = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
sha2 = { workspace = true }
thiserror = { workspace = true }
time = { workspace = true }
url = { workspace = true }
yurt-pkg-format = { path = "../yurt-pkg-format" }
yurt-pkg-trust = { path = "../yurt-pkg-trust" }

[dev-dependencies]
tempfile = { workspace = true }
```

- [ ] **Step 2: Write metadata tests**

Create `crates/yurt-pkg-repo/tests/metadata.rs`:

```rust
use time::OffsetDateTime;
use yurt_pkg_repo::metadata::{Freshness, Index, PackageFile};

#[test]
fn index_rejects_rollback_and_expired_metadata() {
    let json = r#"{
      "schema": 1,
      "index_version": 10,
      "generated_at": "2026-05-07T12:00:00Z",
      "expires_at": "2026-05-14T12:00:00Z",
      "packages": {
        "foo": {"sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "size": 123, "url": "packages/foo.json"}
      }
    }"#;
    let index: Index = serde_json::from_str(json).unwrap();
    let now = OffsetDateTime::parse(
        "2026-05-08T00:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    index.validate_against(Some(9), now, Freshness::default()).unwrap();
    assert!(index.validate_against(Some(10), now, Freshness::default()).is_err());

    let late = OffsetDateTime::parse(
        "2026-06-20T00:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    assert!(index.validate_against(Some(9), late, Freshness::default()).is_err());
}

#[test]
fn package_file_validates_signing_and_dependencies() {
    let json = r#"{
      "name": "foo",
      "versions": [{
        "version": "1.0.0",
        "build": "yurt_0",
        "url": "https://github.com/YurtOS/yurt-packages/releases/download/foo-1.0.0/foo-1.0.0.yurtpkg",
        "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "size": 56789,
        "signing": {
          "subject": "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main",
          "issuer": "https://token.actions.githubusercontent.com"
        },
        "depends": [{"name": "libfoo", "req": "^1.2"}],
        "yanked": false
      }]
    }"#;
    let package: PackageFile = serde_json::from_str(json).unwrap();
    package.validate().unwrap();
    assert_eq!(package.versions[0].depends[0].name, "libfoo");
}
```

- [ ] **Step 3: Run tests and verify failure**

Run: `cargo test -p yurt-pkg-repo --test metadata`

Expected: FAIL because metadata types do not exist.

- [ ] **Step 4: Implement crate module exports**

Create `crates/yurt-pkg-repo/src/lib.rs`:

```rust
pub mod metadata;

pub use metadata::{Index, PackageFile, PackageVersion, RepoPackage};
```

- [ ] **Step 5: Implement metadata types**

Create `crates/yurt-pkg-repo/src/metadata.rs` with:

```rust
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{Duration, OffsetDateTime};
use url::Url;
use yurt_pkg_format::{validate_package_name, Depends};
use yurt_pkg_trust::SigningIdentity;

#[derive(Debug, Error)]
pub enum Error {
    #[error("unsupported schema {0}")]
    UnsupportedSchema(u32),
    #[error("index rollback: new version {new_version} is not greater than cached {cached_version}")]
    Rollback { new_version: u64, cached_version: u64 },
    #[error("index expired at {expires_at}")]
    Expired { expires_at: OffsetDateTime },
    #[error("invalid package name '{0}'")]
    InvalidPackageName(String),
    #[error("invalid package entry url for '{0}': {1}")]
    InvalidUrl(String, url::ParseError),
    #[error("invalid sha256 for '{0}'")]
    InvalidSha256(String),
    #[error("package file name '{file_name}' does not match entry name '{version_name}'")]
    NameMismatch { file_name: String, version_name: String },
    #[error("invalid dependency in '{package}': {message}")]
    InvalidDependency { package: String, message: String },
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy)]
pub struct Freshness {
    pub grace: Duration,
}

impl Default for Freshness {
    fn default() -> Self {
        Self {
            grace: Duration::days(30),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Index {
    pub schema: u32,
    pub index_version: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub generated_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub packages: BTreeMap<String, RepoPackage>,
}

impl Index {
    pub fn validate_against(
        &self,
        cached_version: Option<u64>,
        now: OffsetDateTime,
        freshness: Freshness,
    ) -> Result<()> {
        if self.schema != 1 {
            return Err(Error::UnsupportedSchema(self.schema));
        }
        if let Some(cached_version) = cached_version {
            if self.index_version <= cached_version {
                return Err(Error::Rollback {
                    new_version: self.index_version,
                    cached_version,
                });
            }
        }
        if now > self.expires_at + freshness.grace {
            return Err(Error::Expired {
                expires_at: self.expires_at,
            });
        }
        for (name, package) in &self.packages {
            validate_package_name(name).map_err(|_| Error::InvalidPackageName(name.clone()))?;
            package.validate(name)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoPackage {
    pub sha256: String,
    pub size: u64,
    pub url: String,
}

impl RepoPackage {
    fn validate(&self, name: &str) -> Result<()> {
        validate_sha256(name, &self.sha256)?;
        if !self.url.starts_with("packages/") {
            Url::parse(&self.url).map_err(|err| Error::InvalidUrl(name.to_string(), err))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageFile {
    pub name: String,
    pub versions: Vec<PackageVersion>,
}

impl PackageFile {
    pub fn validate(&self) -> Result<()> {
        validate_package_name(&self.name).map_err(|_| Error::InvalidPackageName(self.name.clone()))?;
        for version in &self.versions {
            if version.name.as_deref().is_some_and(|name| name != self.name) {
                return Err(Error::NameMismatch {
                    file_name: self.name.clone(),
                    version_name: version.name.clone().unwrap(),
                });
            }
            version.validate(&self.name)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageVersion {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub version: String,
    pub build: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub signing: SigningIdentity,
    #[serde(default)]
    pub depends: Vec<Depends>,
    pub yanked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yanked_reason: Option<String>,
}

impl PackageVersion {
    fn validate(&self, package: &str) -> Result<()> {
        semver::Version::parse(&self.version).map_err(|err| Error::InvalidDependency {
            package: package.to_string(),
            message: format!("invalid package version '{}': {err}", self.version),
        })?;
        Url::parse(&self.url).map_err(|err| Error::InvalidUrl(package.to_string(), err))?;
        validate_sha256(package, &self.sha256)?;
        for dep in &self.depends {
            dep.validate().map_err(|err| Error::InvalidDependency {
                package: package.to_string(),
                message: err.to_string(),
            })?;
        }
        Ok(())
    }
}

fn validate_sha256(name: &str, value: &str) -> Result<()> {
    if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(Error::InvalidSha256(name.to_string()))
    }
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p yurt-pkg-repo --test metadata`

Expected: PASS.

- [ ] **Step 7: Commit**

Run:

```bash
git add Cargo.toml crates/yurt-pkg-repo
git commit -m "feat(repo): add signed repository metadata types"
```

### Task 6: Add Cache Diffing and Multi-Repo Selection

**Files:**
- Create: `crates/yurt-pkg-repo/src/cache.rs`
- Create: `crates/yurt-pkg-repo/src/select.rs`
- Modify: `crates/yurt-pkg-repo/src/lib.rs`
- Create: `crates/yurt-pkg-repo/tests/cache.rs`

- [ ] **Step 1: Write cache and selection tests**

Create `crates/yurt-pkg-repo/tests/cache.rs`:

```rust
use std::collections::BTreeMap;

use yurt_pkg_repo::cache::changed_packages;
use yurt_pkg_repo::metadata::RepoPackage;
use yurt_pkg_repo::select::{select_repo_for_package, Candidate};

fn pkg(hash: &str) -> RepoPackage {
    RepoPackage {
        sha256: hash.to_string(),
        size: 1,
        url: "packages/foo.json".to_string(),
    }
}

#[test]
fn package_diff_returns_new_changed_and_removed_names() {
    let mut old = BTreeMap::new();
    old.insert("foo".to_string(), pkg("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    old.insert("old".to_string(), pkg("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"));

    let mut new = BTreeMap::new();
    new.insert("foo".to_string(), pkg("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"));
    new.insert("bar".to_string(), pkg("dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"));

    let diff = changed_packages(&old, &new);
    assert_eq!(diff.changed, vec!["bar".to_string(), "foo".to_string()]);
    assert_eq!(diff.removed, vec!["old".to_string()]);
}

#[test]
fn selection_prefers_lowest_priority_then_repo_id() {
    let candidates = vec![
        Candidate { repo_id: "z".to_string(), priority: 10 },
        Candidate { repo_id: "a".to_string(), priority: 0 },
        Candidate { repo_id: "b".to_string(), priority: 0 },
    ];
    let selected = select_repo_for_package(candidates.iter()).unwrap();
    assert_eq!(selected.repo_id, "a");
}
```

- [ ] **Step 2: Run tests and verify failure**

Run: `cargo test -p yurt-pkg-repo --test cache`

Expected: FAIL because modules do not exist.

- [ ] **Step 3: Export modules**

Update `crates/yurt-pkg-repo/src/lib.rs`:

```rust
pub mod cache;
pub mod metadata;
pub mod select;

pub use metadata::{Index, PackageFile, PackageVersion, RepoPackage};
```

- [ ] **Step 4: Implement cache diffing**

Create `crates/yurt-pkg-repo/src/cache.rs`:

```rust
use std::collections::BTreeMap;

use crate::metadata::RepoPackage;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageDiff {
    pub changed: Vec<String>,
    pub removed: Vec<String>,
}

pub fn changed_packages(
    old: &BTreeMap<String, RepoPackage>,
    new: &BTreeMap<String, RepoPackage>,
) -> PackageDiff {
    let mut changed = Vec::new();
    for (name, new_pkg) in new {
        if old.get(name).map(|old_pkg| old_pkg.sha256.as_str()) != Some(new_pkg.sha256.as_str()) {
            changed.push(name.clone());
        }
    }

    let mut removed = Vec::new();
    for name in old.keys() {
        if !new.contains_key(name) {
            removed.push(name.clone());
        }
    }

    PackageDiff { changed, removed }
}
```

- [ ] **Step 5: Implement selection policy**

Create `crates/yurt-pkg-repo/src/select.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub repo_id: String,
    pub priority: i64,
}

pub fn select_repo_for_package<'a>(
    candidates: impl Iterator<Item = &'a Candidate>,
) -> Option<&'a Candidate> {
    candidates.min_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.repo_id.cmp(&right.repo_id))
    })
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p yurt-pkg-repo --test cache`

Expected: PASS.

- [ ] **Step 7: Commit**

Run:

```bash
git add crates/yurt-pkg-repo/src crates/yurt-pkg-repo/tests/cache.rs
git commit -m "feat(repo): diff package metadata and define repo priority"
```

## Phase 4: `pkg` Command Skeleton

### Task 7: Create `pkg` CLI Surface

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/pkg/Cargo.toml`
- Create: `crates/pkg/src/main.rs`
- Create: `crates/pkg/tests/cli.rs`

- [ ] **Step 1: Add workspace member and create crate manifest**

Add `"crates/pkg"` to the root `Cargo.toml` workspace `members` list.

Create `crates/pkg/Cargo.toml`:

```toml
[package]
name = "pkg"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
anyhow = { workspace = true }
clap = { workspace = true }
yurt-pkg-trust = { path = "../yurt-pkg-trust" }

[dev-dependencies]
assert_cmd = { workspace = true }
predicates = { workspace = true }
```

- [ ] **Step 2: Write CLI tests**

Create `crates/pkg/tests/cli.rs`:

```rust
use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn add_repo_requires_subject_and_issuer() {
    let mut cmd = Command::cargo_bin("pkg").unwrap();
    cmd.args(["add-repo", "https://example.com/repo", "--signing-subject", "subject"]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("--signing-issuer"));
}

#[test]
fn install_reports_planner_boundary() {
    let mut cmd = Command::cargo_bin("pkg").unwrap();
    cmd.args(["install", "foo"]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("resolver/installer spec"));
}
```

- [ ] **Step 3: Run tests and verify failure**

Run: `cargo test -p pkg --test cli`

Expected: FAIL because the crate manifest exists but `src/main.rs` has not been created yet.

- [ ] **Step 4: Implement CLI skeleton**

Create `crates/pkg/src/main.rs`:

```rust
use anyhow::{bail, Result};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "pkg", about = "Yurt package client", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Update,
    Search { query: String },
    Info { name: String },
    Install { spec: String },
    Upgrade { names: Vec<String> },
    Remove { name: String },
    List {
        #[arg(long)]
        yanked: bool,
    },
    AddRepo {
        url: String,
        #[arg(long)]
        signing_subject: String,
        #[arg(long)]
        signing_issuer: String,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        priority: Option<i64>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Update => bail!("pkg update is deferred to the update-flow spec"),
        Command::Search { .. } | Command::Info { .. } => {
            bail!("pkg search/info require db.sqlite cache implementation")
        }
        Command::Install { .. } | Command::Upgrade { .. } => {
            bail!("install and upgrade planning are deferred to the resolver/installer spec")
        }
        Command::Remove { .. } => bail!("remove is deferred to the resolver/installer spec"),
        Command::List { .. } => bail!("list requires installed.sqlite implementation"),
        Command::AddRepo { .. } => bail!("add-repo requires repo:write capability integration"),
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p pkg --test cli`

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
git add Cargo.toml crates/pkg
git commit -m "feat(pkg): add command surface"
```

## Phase 5: Repository CI Tooling

### Task 8: Create Recipe Linter

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/yurt-repo-ci/Cargo.toml`
- Create: `crates/yurt-repo-ci/src/main.rs`
- Create: `crates/yurt-repo-ci/tests/recipe_lint.rs`

- [ ] **Step 1: Add workspace member and create crate manifest**

Add `"crates/yurt-repo-ci"` to the root `Cargo.toml` workspace `members` list.

Create `crates/yurt-repo-ci/Cargo.toml`:

```toml
[package]
name = "yurt-repo-ci"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
anyhow = { workspace = true }
clap = { workspace = true }
semver = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
toml = { workspace = true }
yurt-pkg-format = { path = "../yurt-pkg-format" }
yurt-pkg-repo = { path = "../yurt-pkg-repo" }
yurt-pkg-trust = { path = "../yurt-pkg-trust" }

[dev-dependencies]
assert_cmd = { workspace = true }
predicates = { workspace = true }
tempfile = { workspace = true }
```

- [ ] **Step 2: Write linter tests**

Create `crates/yurt-repo-ci/tests/recipe_lint.rs`:

```rust
use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn lint_accepts_v1_repo_signing_identity() {
    let dir = tempfile::tempdir().unwrap();
    let recipe = dir.path().join("recipe.toml");
    fs::write(&recipe, recipe_text("https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main")).unwrap();

    let mut cmd = Command::cargo_bin("yurt-repo-ci").unwrap();
    cmd.args(["lint-recipe", recipe.to_str().unwrap()]);
    cmd.assert().success();
}

#[test]
fn lint_rejects_other_v1_signing_identity() {
    let dir = tempfile::tempdir().unwrap();
    let recipe = dir.path().join("recipe.toml");
    fs::write(&recipe, recipe_text("https://example.com/other")).unwrap();

    let mut cmd = Command::cargo_bin("yurt-repo-ci").unwrap();
    cmd.args(["lint-recipe", recipe.to_str().unwrap()]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("v1 signing subject"));
}

#[test]
fn lint_continuity_rejects_signer_change_without_migration() {
    let dir = tempfile::tempdir().unwrap();
    let package = dir.path().join("foo.json");
    fs::write(
        &package,
        r#"{
          "name": "foo",
          "versions": [{
            "version": "1.0.0",
            "build": "yurt_0",
            "url": "https://github.com/YurtOS/yurt-packages/releases/download/foo-1.0.0/foo-1.0.0.yurtpkg",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "size": 1,
            "signing": {"subject": "old-subject", "issuer": "https://token.actions.githubusercontent.com"},
            "yanked": false
          }]
        }"#,
    )
    .unwrap();
    let recipe = dir.path().join("recipe.toml");
    fs::write(&recipe, recipe_text("https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main")).unwrap();

    let mut cmd = Command::cargo_bin("yurt-repo-ci").unwrap();
    cmd.args([
        "lint-continuity",
        "--package-file",
        package.to_str().unwrap(),
        "--recipe",
        recipe.to_str().unwrap(),
    ]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("signer continuity"));
}

#[test]
fn lint_continuity_allows_matching_signer() {
    let dir = tempfile::tempdir().unwrap();
    let package = dir.path().join("foo.json");
    fs::write(
        &package,
        r#"{
          "name": "foo",
          "versions": [{
            "version": "1.0.0",
            "build": "yurt_0",
            "url": "https://github.com/YurtOS/yurt-packages/releases/download/foo-1.0.0/foo-1.0.0.yurtpkg",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "size": 1,
            "signing": {
              "subject": "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main",
              "issuer": "https://token.actions.githubusercontent.com"
            },
            "yanked": false
          }]
        }"#,
    )
    .unwrap();
    let recipe = dir.path().join("recipe.toml");
    fs::write(&recipe, recipe_text("https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main")).unwrap();

    let mut cmd = Command::cargo_bin("yurt-repo-ci").unwrap();
    cmd.args([
        "lint-continuity",
        "--package-file",
        package.to_str().unwrap(),
        "--recipe",
        recipe.to_str().unwrap(),
    ]);
    cmd.assert().success();
}

fn recipe_text(subject: &str) -> String {
    format!(
        r#"
[source]
url = "https://example.org/foo-1.0.0.tar.gz"
sha256 = "abc123"

[build]
steps = ["true"]
extended_build = false

[package]
name = "foo"
version = "1.0.0"
build = "yurt_0"
platform = "wasm32-wasip1-yurt"
summary = "Foo"
license = "MIT"
default_uid = 0
default_gid = 0

[package.depends]
libfoo = "^1.2"

[package.signing]
subject = "{subject}"
issuer = "https://token.actions.githubusercontent.com"
"#
    )
}
```

- [ ] **Step 3: Run tests and verify failure**

Run: `cargo test -p yurt-repo-ci --test recipe_lint`

Expected: FAIL because the crate manifest exists but `src/main.rs` has not been created yet.

- [ ] **Step 4: Implement recipe linter**

Create `crates/yurt-repo-ci/src/main.rs`:

```rust
use std::{collections::BTreeMap, fs, path::PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use yurt_pkg_format::validate_package_name;
use yurt_pkg_repo::metadata::PackageFile;

const V1_SUBJECT: &str =
    "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main";
const V1_ISSUER: &str = "https://token.actions.githubusercontent.com";

#[derive(Debug, Parser)]
#[command(name = "yurt-repo-ci", about = "Yurt package repository CI helper", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    LintRecipe { recipe: PathBuf },
    LintContinuity {
        #[arg(long)]
        package_file: PathBuf,
        #[arg(long)]
        recipe: PathBuf,
        #[arg(long)]
        allow_migration: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::LintRecipe { recipe } => lint_recipe(&recipe),
        Command::LintContinuity {
            package_file,
            recipe,
            allow_migration,
        } => lint_continuity(&package_file, &recipe, allow_migration),
    }
}

fn lint_recipe(path: &PathBuf) -> Result<()> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let recipe: Recipe = toml::from_str(&text).context("parsing recipe TOML")?;
    validate_package_name(&recipe.package.name).map_err(|err| anyhow!(err))?;
    semver::Version::parse(&recipe.package.version)
        .with_context(|| format!("invalid package version '{}'", recipe.package.version))?;
    for (name, req) in &recipe.package.depends {
        validate_package_name(name).map_err(|err| anyhow!(err))?;
        semver::VersionReq::parse(req)
            .with_context(|| format!("invalid dependency requirement '{req}' for '{name}'"))?;
    }
    if recipe.package.signing.subject != V1_SUBJECT {
        bail!("v1 signing subject must be {V1_SUBJECT}");
    }
    if recipe.package.signing.issuer != V1_ISSUER {
        bail!("v1 signing issuer must be {V1_ISSUER}");
    }
    Ok(())
}

fn lint_continuity(package_file: &PathBuf, recipe_path: &PathBuf, allow_migration: bool) -> Result<()> {
    let package_text = fs::read_to_string(package_file)
        .with_context(|| format!("reading {}", package_file.display()))?;
    let package: PackageFile = serde_json::from_str(&package_text).context("parsing package JSON")?;
    package.validate().map_err(|err| anyhow!(err))?;

    let recipe_text = fs::read_to_string(recipe_path)
        .with_context(|| format!("reading {}", recipe_path.display()))?;
    let recipe: Recipe = toml::from_str(&recipe_text).context("parsing recipe TOML")?;
    let latest = package
        .versions
        .iter()
        .max_by(|left, right| {
            semver::Version::parse(&left.version)
                .unwrap()
                .cmp(&semver::Version::parse(&right.version).unwrap())
        })
        .context("package file has no versions")?;

    if latest.signing.subject != recipe.package.signing.subject
        || latest.signing.issuer != recipe.package.signing.issuer
    {
        if allow_migration {
            return Ok(());
        }
        bail!(
            "signer continuity violation: latest version uses {} / {}, recipe proposes {} / {}",
            latest.signing.subject,
            latest.signing.issuer,
            recipe.package.signing.subject,
            recipe.package.signing.issuer
        );
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct Recipe {
    package: Package,
}

#[derive(Debug, Deserialize)]
struct Package {
    name: String,
    version: String,
    #[serde(default)]
    depends: BTreeMap<String, String>,
    signing: Signing,
}

#[derive(Debug, Deserialize)]
struct Signing {
    subject: String,
    issuer: String,
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p yurt-repo-ci --test recipe_lint`

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
git add Cargo.toml crates/yurt-repo-ci
git commit -m "feat(repo-ci): lint package recipes"
```

## Phase 6: Verification Boundary and Deferred Spec Stub

### Task 9: Add Verifier Trait and Test Verifier

**Files:**
- Create: `crates/yurt-pkg-repo/src/verify.rs`
- Modify: `crates/yurt-pkg-repo/src/lib.rs`

- [ ] **Step 1: Add tests inside `verify.rs`**

Create `crates/yurt-pkg-repo/src/verify.rs` with this full file:

```rust
use thiserror::Error;
use time::OffsetDateTime;
use yurt_pkg_trust::{SigningIdentity, TrustRoot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationInput<'a> {
    pub payload: &'a [u8],
    pub bundle: &'a [u8],
    pub expected_signing: &'a SigningIdentity,
    pub trust_root: &'a TrustRoot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationOutput {
    pub integrated_time: OffsetDateTime,
    pub subject: String,
    pub issuer: String,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("signing identity mismatch: expected {expected_subject} / {expected_issuer}, got {actual_subject} / {actual_issuer}")]
    SigningIdentityMismatch {
        expected_subject: String,
        expected_issuer: String,
        actual_subject: String,
        actual_issuer: String,
    },
    #[error("bundle verification is not wired to sigstore yet")]
    NotImplemented,
}

pub type Result<T> = std::result::Result<T, Error>;

pub trait BundleVerifier {
    fn verify(&self, input: VerificationInput<'_>) -> Result<VerificationOutput>;
}

#[derive(Debug, Clone)]
pub struct StaticVerifier {
    pub output: VerificationOutput,
}

impl BundleVerifier for StaticVerifier {
    fn verify(&self, input: VerificationInput<'_>) -> Result<VerificationOutput> {
        if !input
            .expected_signing
            .matches(&self.output.subject, &self.output.issuer)
        {
            return Err(Error::SigningIdentityMismatch {
                expected_subject: input.expected_signing.subject.clone(),
                expected_issuer: input.expected_signing.issuer.clone(),
                actual_subject: self.output.subject.clone(),
                actual_issuer: self.output.issuer.clone(),
            });
        }
        Ok(self.output.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_verifier_checks_subject_and_issuer() {
        let verifier = StaticVerifier {
            output: VerificationOutput {
                integrated_time: OffsetDateTime::UNIX_EPOCH,
                subject: "subject".to_string(),
                issuer: "issuer".to_string(),
            },
        };
        let expected = SigningIdentity {
            subject: "subject".to_string(),
            issuer: "issuer".to_string(),
        };
        let trust_root = TrustRoot::from_dir("/etc/yurt-pkg/sigstore-trust-root");
        verifier
            .verify(VerificationInput {
                payload: b"index",
                bundle: b"bundle",
                expected_signing: &expected,
                trust_root: &trust_root,
            })
            .unwrap();

        let wrong = SigningIdentity {
            subject: "subject".to_string(),
            issuer: "other".to_string(),
        };
        assert!(verifier
            .verify(VerificationInput {
                payload: b"index",
                bundle: b"bundle",
                expected_signing: &wrong,
                trust_root: &trust_root,
            })
            .is_err());
    }
}
```

- [ ] **Step 2: Export verifier module**

Update `crates/yurt-pkg-repo/src/lib.rs`:

```rust
pub mod cache;
pub mod metadata;
pub mod select;
pub mod verify;

pub use metadata::{Index, PackageFile, PackageVersion, RepoPackage};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p yurt-pkg-repo verify`

Expected: PASS.

- [ ] **Step 4: Commit**

Run:

```bash
git add crates/yurt-pkg-repo/src/verify.rs crates/yurt-pkg-repo/src/lib.rs
git commit -m "feat(repo): define bundle verification boundary"
```

### Task 10: Record Resolver/Installer Boundary

**Files:**
- Create: `docs/superpowers/specs/2026-05-07-yurt-pkg-resolver-installer-design.md`
- Create: `docs/superpowers/specs/2026-05-07-yurt-pkg-update-flow-design.md`

- [ ] **Step 1: Create boundary spec stub**

Create `docs/superpowers/specs/2026-05-07-yurt-pkg-resolver-installer-design.md`:

```markdown
# Yurt Package Resolver and Installer Design

**Status:** Stub
**Date:** 2026-05-07

## Scope

This follow-up spec owns resolver algorithm internals, transaction planning,
file ownership collision handling, install root atomicity strategy, installed
database schema, removal semantics, and install hooks.

The distribution design fixes the user-visible contract:

- install and upgrade include transitive dependencies;
- already-installed packages contribute constraints to the solve;
- version collisions abort before filesystem mutation;
- yanked versions are skipped unless explicitly allowed;
- failed install/upgrade transactions leave no partial state visible.

## Open Decisions

- Solver strategy and backtracking order.
- How package repo priority affects dependency selection when multiple trusted
  repos provide the same package name.
- Whether installed packages may move between repos during upgrade.
- Atomic install strategy for the Yurt filesystem.
- `installed.sqlite` schema and migration policy.
```

Create `docs/superpowers/specs/2026-05-07-yurt-pkg-update-flow-design.md`:

```markdown
# Yurt Package Update Flow Design

**Status:** Stub
**Date:** 2026-05-07

## Scope

This follow-up spec owns the executable `pkg update` flow:

- HTTP fetch of `index.json` and `index.json.bundle`;
- ETag and 304 handling;
- re-evaluating cached `expires_at` on 304;
- `meta.json` persistence for `last_fetched`, `last_index_version`,
  `last_integrated_time`, and `consecutive_fetch_failures`;
- package-file downloads and hash verification;
- `db.sqlite` search/info cache schema and updates;
- command integration for `pkg update`, `pkg search`, and `pkg info`.

The distribution implementation plan creates the metadata, freshness,
rollback, trust-root, and verifier boundaries this flow will use, but it
does not implement network/cache persistence.
```

- [ ] **Step 2: Commit**

Run:

```bash
git add docs/superpowers/specs/2026-05-07-yurt-pkg-resolver-installer-design.md docs/superpowers/specs/2026-05-07-yurt-pkg-update-flow-design.md
git commit -m "docs: record deferred client flow scopes"
```

## Phase 7: Full Verification

### Task 11: Run Workspace Checks

**Files:**
- All workspace files changed above.

- [ ] **Step 1: Format**

Run: `cargo fmt --all -- --check`

Expected: PASS. If it fails, run `cargo fmt --all`, inspect `git diff`, and keep the formatting changes.

- [ ] **Step 2: Test**

Run: `cargo test --workspace`

Expected: PASS.

- [ ] **Step 3: Check**

Run: `cargo check --workspace`

Expected: PASS.

- [ ] **Step 4: Review diff**

Run: `git diff --stat yurt-pkg-distribution-plan-base..HEAD`

Expected: shows only the crates and docs listed in this plan.

## Phase 8: Sigstore Smoke Test Decision

### Task 12: Smoke-Test `sigstore` on `wasm32-wasip1`

**Files:**
- Modify after the test: `crates/yurt-pkg-repo/Cargo.toml`
- Modify after the test: `crates/yurt-pkg-repo/src/verify.rs`

- [ ] **Step 1: Add target**

Run: `rustup target add wasm32-wasip1`

Expected: target installed or already installed.

- [ ] **Step 2: Try upstream Sigstore dependency**

Add the current `sigstore` crate to `crates/yurt-pkg-repo/Cargo.toml`:

```toml
sigstore = { version = "0.11", default-features = false }
```

Run: `cargo check -p yurt-pkg-repo --target wasm32-wasip1`

Expected: one of:

- PASS: proceed with an upstream-backed `SigstoreVerifier` implementation in a follow-up task.
- FAIL due to unsupported networking, crypto, or OS APIs: remove the `sigstore` dependency and create a follow-up implementation plan for the fallback verifier described in the distribution spec.

- [ ] **Step 3: Commit the decision**

If upstream Sigstore compiles:

```bash
git add crates/yurt-pkg-repo/Cargo.toml Cargo.lock
git commit -m "chore(repo): enable sigstore verifier dependency"
```

If upstream Sigstore does not compile:

```bash
git add crates/yurt-pkg-repo/Cargo.toml Cargo.lock docs/superpowers/plans
git commit -m "docs(repo): record sigstore wasi fallback decision"
```

## Self-Review Checklist

- Spec coverage: package dependency schema, signing subject/issuer, trusted repos, Sigstore trust-root path model, repository index/package metadata, rollback/freshness primitives, bundle sidecar layout, CI recipe linting, signer continuity, command surface, and multi-repo priority are covered.
- Deferred by design: executable `pkg update` HTTP/cache/db flow including 304 handling, resolver internals, install transaction implementation strategy, install hooks, file conflict policy, real revocation lists, and full Sigstore fallback implementation.
- Placeholder scan: no task uses unspecified error handling or unnamed tests; every code-producing step includes concrete code.
- Type consistency: dependency type is `Depends { name, req }`; signing type is `SigningIdentity { subject, issuer }`; repository metadata uses `index_version`, `expires_at`, and `.bundle` throughout.
