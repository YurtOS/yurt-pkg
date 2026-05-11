# Yurt Pkg Resolver Installer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `pkg install` and `pkg list` stubs with a first real installer that resolves cached package metadata, applies file-backed `.yurtpkg` archives into a root, and records installed state.

**Architecture:** Keep repository cache and package archive validation in the existing crates. Add focused modules inside `crates/pkg` for spec parsing, installed-state SQLite, dependency resolution, and install application. The first slice implements `install` and `list`; `upgrade` and `remove` remain explicit stubs but share the installed-state boundaries introduced here.

**Tech Stack:** Rust 1.95, `clap`, `rusqlite`, `semver`, `time`, `url`, `fs2`, `yurt-pkg-repo`, `yurt-pkg-format`.

---

## File Structure

- Modify `crates/pkg/Cargo.toml`: add `rusqlite`, `semver`, `serde_json`, `sha2`, `fs2`, and `yurt-pkg-format` dependencies needed by installer internals.
- Modify `crates/pkg/src/main.rs`: wire `--state-root` and hidden `--root`, dispatch `install` and `list`, and keep command rendering small.
- Create `crates/pkg/src/spec.rs`: parse `name`, `name@version`, and `name@version-yurt_N` specs.
- Create `crates/pkg/src/installed.rs`: own `/var/lib/yurt-pkg` layout, advisory lock, schema creation, recovery cleanup, installed package reads, and installed package writes.
- Create `crates/pkg/src/resolver.rs`: load package metadata from current repo cache, enforce freshness, select candidates, and produce an install plan.
- Create `crates/pkg/src/apply.rs`: download file-backed archives through `LocalFileFetcher`, verify archive metadata and hash, collision-check manifests, stage payload entries, copy into `--root`, and commit installed state.
- Modify `crates/pkg/tests/cli.rs`: add test archive fixture helpers and CLI tests for install/list, dependency ordering, exact pins, collision failures, and installed-version conflicts.

## Task 1: Package Spec Parser

**Files:**
- Create: `crates/pkg/src/spec.rs`
- Modify: `crates/pkg/src/main.rs`
- Test: `crates/pkg/src/spec.rs`

- [ ] **Step 1: Write failing parser tests**

Add `mod spec;` near the top of `crates/pkg/src/main.rs`.

Create `crates/pkg/src/spec.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unversioned_name() {
        let spec = PackageSpec::parse("busybox").unwrap();
        assert_eq!(spec.name, "busybox");
        assert_eq!(spec.version.as_deref(), None);
        assert_eq!(spec.build.as_deref(), None);
    }

    #[test]
    fn parses_semver_prerelease_as_version_not_build() {
        let spec = PackageSpec::parse("foo@1.0.0-rc.1").unwrap();
        assert_eq!(spec.name, "foo");
        assert_eq!(spec.version.as_deref(), Some("1.0.0-rc.1"));
        assert_eq!(spec.build.as_deref(), None);
    }

    #[test]
    fn parses_final_yurt_build_suffix() {
        let spec = PackageSpec::parse("foo@1.0.0-rc.1-yurt_7").unwrap();
        assert_eq!(spec.version.as_deref(), Some("1.0.0-rc.1"));
        assert_eq!(spec.build.as_deref(), Some("yurt_7"));
    }

    #[test]
    fn rejects_invalid_name() {
        let err = PackageSpec::parse("Bad@1.0.0").unwrap_err().to_string();
        assert!(err.contains("invalid package name"));
    }

    #[test]
    fn rejects_invalid_version() {
        let err = PackageSpec::parse("foo@not-semver").unwrap_err().to_string();
        assert!(err.contains("invalid version"));
    }
}
```

- [ ] **Step 2: Run parser tests and verify RED**

Run:

```bash
cargo test -p pkg spec::tests
```

Expected: compile failure because `PackageSpec` does not exist yet.

- [ ] **Step 3: Implement parser minimally**

Add this implementation above the tests:

```rust
use anyhow::{anyhow, Result};
use yurt_pkg_format::validate_package_name;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSpec {
    pub name: String,
    pub version: Option<String>,
    pub build: Option<String>,
}

impl PackageSpec {
    pub fn parse(input: &str) -> Result<Self> {
        let (name, rest) = input
            .split_once('@')
            .map_or((input, None), |(name, rest)| (name, Some(rest)));
        validate_package_name(name).map_err(|err| anyhow!("{err}"))?;
        let Some(rest) = rest else {
            return Ok(Self {
                name: name.to_string(),
                version: None,
                build: None,
            });
        };
        if rest.is_empty() {
            return Err(anyhow!("invalid version in package spec '{input}'"));
        }
        let (version, build) = split_yurt_build(rest);
        semver::Version::parse(version)
            .map_err(|err| anyhow!("invalid version '{version}' in package spec '{input}': {err}"))?;
        Ok(Self {
            name: name.to_string(),
            version: Some(version.to_string()),
            build: build.map(ToOwned::to_owned),
        })
    }
}

fn split_yurt_build(value: &str) -> (&str, Option<&str>) {
    let Some((version, maybe_build)) = value.rsplit_once('-') else {
        return (value, None);
    };
    if yurt_build_number(maybe_build).is_some() {
        (version, Some(maybe_build))
    } else {
        (value, None)
    }
}

pub fn yurt_build_number(build: &str) -> Option<u64> {
    build.strip_prefix("yurt_")?.parse().ok()
}
```

- [ ] **Step 4: Run parser tests and verify GREEN**

Run:

```bash
cargo test -p pkg spec::tests
```

Expected: all parser tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/pkg/Cargo.toml crates/pkg/src/main.rs crates/pkg/src/spec.rs
git commit -m "feat: parse package install specs"
```

## Task 2: Installed State Database

**Files:**
- Create: `crates/pkg/src/installed.rs`
- Modify: `crates/pkg/src/main.rs`
- Test: `crates/pkg/src/installed.rs`

- [ ] **Step 1: Write failing installed-state tests**

Add `mod installed;` near the top of `crates/pkg/src/main.rs`.

Create `crates/pkg/src/installed.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use yurt_pkg_format::{Depends, FileEntry, FileEntryKind};

    #[test]
    fn init_creates_schema_and_list_ignores_prepared() {
        let temp = tempdir().unwrap();
        let store = InstalledStore::open(temp.path()).unwrap();
        store.record_prepared_for_test("tx1", "foo").unwrap();

        assert!(store.list_installed().unwrap().is_empty());
    }

    #[test]
    fn failed_recovery_removes_prepared_children_when_staging_is_missing() {
        let temp = tempdir().unwrap();
        let store = InstalledStore::open(temp.path()).unwrap();
        store.record_prepared_for_test("tx1", "foo").unwrap();

        store.recover_prepared_transactions(temp.path()).unwrap();

        assert!(store.list_installed().unwrap().is_empty());
        assert!(store.path_owner("bin/foo").unwrap().is_none());
    }

    #[test]
    fn prepared_package_is_hidden_until_commit_and_then_owns_non_directory_paths() {
        let temp = tempdir().unwrap();
        let store = InstalledStore::open(temp.path()).unwrap();
        let files = vec![
            FileEntry {
                path: "usr".into(),
                kind: FileEntryKind::Dir,
                sha256: None,
                size: None,
                target: None,
                mode: "0755".into(),
                uid: 0,
                gid: 0,
            },
            FileEntry {
                path: "usr/bin/foo".into(),
                kind: FileEntryKind::File,
                sha256: Some("a".repeat(64)),
                size: Some(1),
                target: None,
                mode: "0755".into(),
                uid: 0,
                gid: 0,
            },
        ];
        let package = InstalledPackageInput::new_for_test(
            "foo",
            "1.0.0",
            "yurt_0",
            files,
            Vec::<Depends>::new(),
        );

        store.prepare_install("tx1", &[package]).unwrap();

        assert!(store.list_installed().unwrap().is_empty());
        assert!(store.path_owner("usr/bin/foo").unwrap().is_none());

        store.commit_prepared_install("tx1").unwrap();

        assert_eq!(store.list_installed().unwrap()[0].name, "foo");
        assert!(store.path_owner("usr").unwrap().is_none());
        assert_eq!(store.path_owner("usr/bin/foo").unwrap().unwrap(), "foo");
    }
}
```

- [ ] **Step 2: Run installed-state tests and verify RED**

Run:

```bash
cargo test -p pkg installed::tests
```

Expected: compile failure because `InstalledStore` and inputs do not exist yet.

- [ ] **Step 3: Implement installed-state schema and helpers**

Implement `InstalledStore` with:

```rust
pub struct InstalledStore {
    root: PathBuf,
    conn: rusqlite::Connection,
}
```

Required public methods:

```rust
impl InstalledStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self>;
    pub fn lock(root: impl AsRef<Path>) -> Result<InstallLock>;
    pub fn recover_prepared_transactions(&self, sandbox_root: &Path) -> Result<()>;
    pub fn list_installed(&self) -> Result<Vec<InstalledPackage>>;
    pub fn installed_packages(&self) -> Result<BTreeMap<String, InstalledPackage>>;
    pub fn path_owner(&self, path: &str) -> Result<Option<String>>;
    pub fn prepare_install(&self, txid: &str, packages: &[InstalledPackageInput]) -> Result<()>;
    pub fn commit_prepared_install(&self, txid: &str) -> Result<()>;
}
```

Use the schema from `docs/superpowers/specs/2026-05-07-yurt-pkg-resolver-installer-design.md`, including:

```sql
CREATE TABLE IF NOT EXISTS transactions (...);
CREATE TABLE IF NOT EXISTS packages (... install_state TEXT NOT NULL CHECK (...));
CREATE TABLE IF NOT EXISTS dependencies (...);
CREATE TABLE IF NOT EXISTS files (...);
CREATE INDEX IF NOT EXISTS transactions_state_idx ON transactions(state);
CREATE INDEX IF NOT EXISTS packages_install_transaction_idx ON packages(install_transaction_id);
CREATE INDEX IF NOT EXISTS files_install_transaction_idx ON files(install_transaction_id);
```

In `prepare_install`, insert the prepared `transactions`, `packages`, `dependencies`, and `files` rows in one SQLite transaction. `pkg list`, `installed_packages`, and `path_owner` must ignore `install_state = 'prepared'` rows during normal queries so a prepared transaction reserves paths only for recovery and the active apply flow, not for user-visible installed state.

In `commit_prepared_install`, set package rows for the transaction to `install_state = 'installed'` and the transaction row to `state = 'committed'` in one SQLite transaction.

In `recover_prepared_transactions`, complete or fail every `transactions.state = 'prepared'` install before new work starts. If the transaction's staging tree is valid, copy staged entries into `sandbox_root` idempotently, then call `commit_prepared_install`. If staging is missing or corrupt, delete that transaction's rows from `files`, `dependencies`, and `packages`, then set the transaction row to `failed` with an error in one SQLite transaction. Prepared rows must never be silently discarded after files may have been copied into `sandbox_root`.

- [ ] **Step 4: Run installed-state tests and verify GREEN**

Run:

```bash
cargo test -p pkg installed::tests
```

Expected: installed-state tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/pkg/Cargo.toml crates/pkg/src/main.rs crates/pkg/src/installed.rs
git commit -m "feat: record installed package state"
```

## Task 3: Resolver Planning

**Files:**
- Create: `crates/pkg/src/resolver.rs`
- Modify: `crates/pkg/src/main.rs`
- Test: `crates/pkg/src/resolver.rs`

- [ ] **Step 1: Write failing resolver tests**

Add `mod resolver;` near the top of `crates/pkg/src/main.rs`.

Create resolver tests that build `PackageRecord` values directly:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_highest_non_yanked_version_and_build() {
        let universe = PackageUniverse::from_records(vec![
            record("official", 0, "foo", "1.0.0", "yurt_0", false, &[]),
            record("official", 0, "foo", "1.0.0", "yurt_2", false, &[]),
            record("official", 0, "foo", "2.0.0", "yurt_0", true, &[]),
        ]);

        let plan = Resolver::new(universe, BTreeMap::new())
            .resolve(&[PackageSpec::parse("foo").unwrap()])
            .unwrap();

        assert_eq!(plan.to_install[0].name, "foo");
        assert_eq!(plan.to_install[0].version, "1.0.0");
        assert_eq!(plan.to_install[0].build, "yurt_2");
    }

    #[test]
    fn installs_dependencies_before_dependents() {
        let universe = PackageUniverse::from_records(vec![
            record("official", 0, "app", "1.0.0", "yurt_0", false, &[("lib", "^1")]),
            record("official", 0, "lib", "1.2.0", "yurt_0", false, &[]),
        ]);

        let plan = Resolver::new(universe, BTreeMap::new())
            .resolve(&[PackageSpec::parse("app").unwrap()])
            .unwrap();

        assert_eq!(plan.to_install.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["lib", "app"]);
    }

    #[test]
    fn installed_version_conflict_fails_install() {
        let universe = PackageUniverse::from_records(vec![
            record("official", 0, "app", "1.0.0", "yurt_0", false, &[("lib", "^2")]),
            record("official", 0, "lib", "2.0.0", "yurt_0", false, &[]),
        ]);
        let installed = BTreeMap::from([("lib".to_string(), installed("lib", "1.0.0", "yurt_0", &[]))]);

        let err = Resolver::new(universe, installed)
            .resolve(&[PackageSpec::parse("app").unwrap()])
            .unwrap_err()
            .to_string();

        assert!(err.contains("installed lib 1.0.0-yurt_0 conflicts"));
    }
}
```

- [ ] **Step 2: Run resolver tests and verify RED**

Run:

```bash
cargo test -p pkg resolver::tests
```

Expected: compile failure because resolver types do not exist.

- [ ] **Step 3: Implement resolver**

Implement:

```rust
pub struct PackageUniverse {
    records_by_name: BTreeMap<String, Vec<PackageRecord>>,
}

pub struct PackageRecord {
    pub repo_id: String,
    pub priority: i64,
    pub name: String,
    pub version: String,
    pub build: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub signing: SigningIdentity,
    pub depends: Vec<Depends>,
    pub yanked: bool,
    pub package_json: PackageFile,
}

pub struct InstallPlan {
    pub to_install: Vec<PackageRecord>,
    pub reused: Vec<InstalledPackage>,
}

pub struct Resolver {
    universe: PackageUniverse,
    installed: BTreeMap<String, InstalledPackage>,
}
```

Implement deterministic candidate ordering:

1. repo priority ascending;
2. repo id ascending;
3. SemVer version descending;
4. Yurt build number descending.

Implement enough backtracking for v1 by recomputing unresolved requirements after each candidate choice, and explicitly failing if a fixed installed package does not satisfy accumulated requirements. Keep the implementation private to `resolver.rs`; only expose `load_universe_from_cache` and `Resolver::resolve`.

- [ ] **Step 4: Run resolver tests and verify GREEN**

Run:

```bash
cargo test -p pkg resolver::tests
```

Expected: resolver tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/pkg/src/main.rs crates/pkg/src/resolver.rs
git commit -m "feat: resolve package install plans"
```

## Task 4: Archive Apply And Collision Checks

**Files:**
- Create: `crates/pkg/src/apply.rs`
- Modify: `crates/pkg/src/main.rs`
- Test: `crates/pkg/src/apply.rs`

- [ ] **Step 1: Write failing apply tests**

Add `mod apply;` near the top of `crates/pkg/src/main.rs`.

Write tests that use `yurt_pkg_format::Writer` to build `.yurtpkg` bytes:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn applies_archive_and_records_file_owner() {
        let state = tempdir().unwrap();
        let root = tempdir().unwrap();
        let store = InstalledStore::open(state.path()).unwrap();
        let package = planned_package_with_archive("hello", "1.0.0", "yurt_0", archive_with_file("bin/hello", b"hello\n"));

        apply_plan(root.path(), state.path(), &store, &[package]).unwrap();

        assert_eq!(std::fs::read(root.path().join("bin/hello")).unwrap(), b"hello\n");
        assert_eq!(store.path_owner("bin/hello").unwrap().unwrap(), "hello");
    }

    #[test]
    fn rejects_file_directory_collision() {
        let state = tempdir().unwrap();
        let root = tempdir().unwrap();
        let store = InstalledStore::open(state.path()).unwrap();
        let packages = vec![
            planned_package_with_archive("a", "1.0.0", "yurt_0", archive_with_dir("share/foo")),
            planned_package_with_archive("b", "1.0.0", "yurt_0", archive_with_file("share/foo", b"x")),
        ];

        let err = apply_plan(root.path(), state.path(), &store, &packages).unwrap_err().to_string();

        assert!(err.contains("collides with directory path share/foo"));
    }
}
```

- [ ] **Step 2: Run apply tests and verify RED**

Run:

```bash
cargo test -p pkg apply::tests
```

Expected: compile failure because apply APIs do not exist.

- [ ] **Step 3: Implement apply**

Implement:

```rust
pub fn apply_plan(root: &Path, state_root: &Path, store: &InstalledStore, packages: &[PlannedArchive]) -> Result<()>;
```

Core behavior:

- read archive bytes from `file://` URLs with `LocalFileFetcher`;
- check `size` and `sha256`;
- parse with `yurt_pkg_format::Reader`;
- compare archive `index` name/version/build and semantic dependencies against selected metadata;
- build all directory paths from every archive `files_json`;
- reject file/symlink/hardlink collisions with installed `files`, in-transaction `files`, and installed or in-transaction directory paths;
- stage entries under `<state_root>/staging/<txid>/root`;
- call `store.prepare_install(txid, packages)` before mutating `root`, so a crash after this point has recoverable ownership state;
- copy staged entries into `root`;
- call `store.commit_prepared_install(txid)` after successful copy.

The apply order must match the spec's database-first recovery model. A failure before `prepare_install` leaves only staging data and no visible installed state; a failure after `prepare_install` leaves a prepared transaction that recovery can either complete from staging or fail and remove from the ownership tables. Do not copy into `root` before the prepared transaction, package, dependency, and file rows have been recorded.

Use `std::os::unix::fs::symlink` and `std::fs::hard_link` for links. Set permissions with `std::fs::set_permissions` using `PermissionsExt::from_mode`.

- [ ] **Step 4: Run apply tests and verify GREEN**

Run:

```bash
cargo test -p pkg apply::tests
```

Expected: apply tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/pkg/Cargo.toml crates/pkg/src/main.rs crates/pkg/src/apply.rs
git commit -m "feat: apply package install transactions"
```

## Task 5: CLI Install And List

**Files:**
- Modify: `crates/pkg/src/main.rs`
- Modify: `crates/pkg/tests/cli.rs`

- [ ] **Step 1: Write failing CLI tests**

Update `crates/pkg/tests/cli.rs`:

```rust
#[test]
fn cli_install_file_backed_package_and_list_it() {
    let fixture = RepoFixture::new_with_archive_package();
    fixture.populate_cache();
    let root = tempdir().unwrap();
    let state = tempdir().unwrap();

    let mut install = Command::cargo_bin("pkg").unwrap();
    install.args([
        "--etc-root",
        fixture.etc.path().to_str().unwrap(),
        "--cache-root",
        fixture.cache.path().to_str().unwrap(),
        "--state-root",
        state.path().to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "install",
        "tool",
    ]);
    install.assert().success().stdout(predicate::str::contains("install tool 1.0.0-yurt_0"));

    assert_eq!(fs::read(root.path().join("bin/tool")).unwrap(), b"tool\n");

    let mut list = Command::cargo_bin("pkg").unwrap();
    list.args([
        "--etc-root",
        fixture.etc.path().to_str().unwrap(),
        "--cache-root",
        fixture.cache.path().to_str().unwrap(),
        "--state-root",
        state.path().to_str().unwrap(),
        "list",
    ]);
    list.assert().success().stdout(predicate::str::contains("tool 1.0.0-yurt_0 official"));
}
```

- [ ] **Step 2: Run CLI test and verify RED**

Run:

```bash
cargo test -p pkg --test cli cli_install_file_backed_package_and_list_it
```

Expected: failure because `--state-root`, `--root`, or real install behavior is missing.

- [ ] **Step 3: Wire CLI**

In `Cli`, add hidden args:

```rust
#[arg(long, hide = true, default_value = "/var/lib/yurt-pkg")]
state_root: PathBuf,
#[arg(long, hide = true, default_value = "/")]
root: PathBuf,
```

Change `Install` to accept one or more specs:

```rust
Install { spec: Vec<String> }
```

Dispatch:

```rust
Command::Install { spec } => install(&cli.etc_root, &cli.cache_root, &cli.state_root, &cli.root, &spec),
Command::List { .. } => list(&cli.state_root),
```

Implement `install` by:

1. parsing specs;
2. loading trusted repos and current cache package metadata;
3. taking installed-state lock;
4. opening installed DB;
5. running recovery;
6. resolving plan;
7. printing plan lines;
8. applying plan.

Implement `list` by opening `InstalledStore` and printing `name version-build repo_id`.

- [ ] **Step 4: Run CLI test and verify GREEN**

Run:

```bash
cargo test -p pkg --test cli cli_install_file_backed_package_and_list_it
```

Expected: CLI test passes.

- [ ] **Step 5: Run broader package tests**

Run:

```bash
cargo test -p pkg --tests
```

Expected: all `pkg` tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/pkg/src/main.rs crates/pkg/tests/cli.rs
git commit -m "feat: install packages from cached repos"
```

## Task 6: Acceptance And Edge Tests

**Files:**
- Modify: `crates/pkg/tests/cli.rs`
- Optional Modify: `docs/superpowers/specs/2026-05-07-yurt-pkg-resolver-installer-design.md` only if implementation reveals a spec ambiguity.

- [ ] **Step 1: Add CLI edge tests**

Add these CLI tests:

```rust
#[test]
fn cli_install_exact_version_build_pin_selects_that_build() {
    let fixture = RepoFixture::new_with_archive_versions(&[
        ("tool", "1.0.0", "yurt_0", b"old\n"),
        ("tool", "1.0.0", "yurt_1", b"new\n"),
    ]);
    fixture.populate_cache();
    let root = tempdir().unwrap();
    let state = tempdir().unwrap();

    let mut cmd = Command::cargo_bin("pkg").unwrap();
    cmd.args([
        "--etc-root",
        fixture.etc.path().to_str().unwrap(),
        "--cache-root",
        fixture.cache.path().to_str().unwrap(),
        "--state-root",
        state.path().to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "install",
        "tool@1.0.0-yurt_0",
    ]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("install tool 1.0.0-yurt_0"));
    assert_eq!(fs::read(root.path().join("bin/tool")).unwrap(), b"old\n");
}

#[test]
fn cli_install_refuses_installed_version_change() {
    let fixture = RepoFixture::new_with_dependency_conflict();
    fixture.populate_cache();
    let root = tempdir().unwrap();
    let state = tempdir().unwrap();

    let mut first = Command::cargo_bin("pkg").unwrap();
    first.args([
        "--etc-root",
        fixture.etc.path().to_str().unwrap(),
        "--cache-root",
        fixture.cache.path().to_str().unwrap(),
        "--state-root",
        state.path().to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "install",
        "lib@1.0.0-yurt_0",
    ]);
    first.assert().success();

    let mut second = Command::cargo_bin("pkg").unwrap();
    second.args([
        "--etc-root",
        fixture.etc.path().to_str().unwrap(),
        "--cache-root",
        fixture.cache.path().to_str().unwrap(),
        "--state-root",
        state.path().to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "install",
        "app",
    ]);
    second
        .assert()
        .failure()
        .stderr(predicate::str::contains("installed lib 1.0.0-yurt_0 conflicts"));
}

#[test]
fn cli_install_refuses_unmanaged_existing_path() {
    let fixture = RepoFixture::new_with_archive_package();
    fixture.populate_cache();
    let root = tempdir().unwrap();
    let state = tempdir().unwrap();
    fs::create_dir_all(root.path().join("bin")).unwrap();
    fs::write(root.path().join("bin/tool"), b"local\n").unwrap();

    let mut cmd = Command::cargo_bin("pkg").unwrap();
    cmd.args([
        "--etc-root",
        fixture.etc.path().to_str().unwrap(),
        "--cache-root",
        fixture.cache.path().to_str().unwrap(),
        "--state-root",
        state.path().to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "install",
        "tool",
    ]);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("would overwrite unmanaged path bin/tool"));
}

#[test]
fn cli_install_refuses_stale_cache_past_grace() {
    let fixture = RepoFixture::new_with_archive_package();
    fixture.populate_cache();
    fixture.make_cache_stale_with_failures();
    let root = tempdir().unwrap();
    let state = tempdir().unwrap();

    let mut cmd = Command::cargo_bin("pkg").unwrap();
    cmd.args([
        "--etc-root",
        fixture.etc.path().to_str().unwrap(),
        "--cache-root",
        fixture.cache.path().to_str().unwrap(),
        "--state-root",
        state.path().to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "install",
        "tool",
    ]);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("cache is stale; run pkg update"));
}
```

- [ ] **Step 2: Run edge tests and verify RED where behavior is missing**

Run each test by name, for example:

```bash
cargo test -p pkg --test cli cli_install_refuses_unmanaged_existing_path
```

Expected: each new test fails until the missing behavior is implemented.

- [ ] **Step 3: Fill missing behavior minimally**

Only add implementation required by these tests. Do not implement `upgrade`, `remove`, yanked `--allow-yanked`, interactive confirmation, or HTTP archive fetch in this slice.

- [ ] **Step 4: Run all local gates**

Run:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --tests
```

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/pkg docs/superpowers/specs/2026-05-07-yurt-pkg-resolver-installer-design.md
git commit -m "test: cover installer edge cases"
```

## Self-Review Notes

- Spec coverage: package spec parsing, conservative install semantics, installed-state schema, recovery cleanup, cache freshness, collision checks, and `pkg list` are covered. `pkg upgrade`, `pkg remove`, yanked `--allow-yanked`, HTTP archive fetch, and stronger filesystem atomicity are explicitly out of this first implementation slice.
- Placeholder scan: no step uses `TBD`, `TODO`, or unspecified "add tests" wording without naming the behavior.
- Type consistency: `PackageSpec`, `InstalledStore`, `Resolver`, `InstallPlan`, and `apply_plan` are introduced before later tasks depend on them.
