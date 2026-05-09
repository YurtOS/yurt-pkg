# Yurt Package Resolver and Installer Design

**Status:** Draft
**Date:** 2026-05-07
**Scope:** Dependency resolution, install transaction planning, archive
verification, file ownership checks, installed package state, and the first
real `pkg install` behavior.

## Summary

`pkg update`, `pkg search`, and `pkg info` already establish the trusted
repository cache. This spec turns that cache into an installer. The core
mechanism is intentionally conventional:

1. Parse package specs such as `busybox` and `busybox@1.37.0`.
2. Resolve the complete dependency closure from cached repositories.
3. Treat installed packages as constraints. `install` may add missing packages
   but must not silently change an installed package's version.
4. Download and verify every selected archive before mutating the filesystem.
5. Detect file ownership conflicts before mutation.
6. Apply packages in dependency order and record installed manifests.

BusyBox is only an acceptance package. The design target is the general package
manager mechanism.

## Existing Inputs

Trusted repository cache:

```text
/etc/yurt-pkg/trusted-repos.toml
/var/cache/yurt-pkg/repos/<repo-id>/current -> snapshots/<snapshot-id>/
/var/cache/yurt-pkg/repos/<repo-id>/snapshots/<snapshot-id>/
  index.json
  packages/<name>.json
  db.sqlite
```

Package archive format:

```text
info/index.json
info/files.json
info/yurt.json
bin/, usr/, etc/, ...
```

Repository package versions declare:

- `version`: upstream SemVer version.
- `build`: Yurt build metadata such as `yurt_0`.
- `depends`: SemVer requirements over package names.
- `url`, `sha256`, `size`, `signing`: archive download and verification data.
- `yanked`: versions skipped by default.

## Package Spec Syntax

`pkg install` accepts one or more package specs:

```text
pkg install busybox
pkg install busybox@1.37.0
pkg install busybox@1.37.0-yurt_0
```

Rules:

- `<name>` installs the latest non-yanked package version satisfying all
  transitive constraints.
- `<name>@<version>` pins the upstream SemVer version. If multiple builds exist
  for that version, the resolver chooses the highest build by Yurt build order.
- `<name>@<version>-<build>` pins both version and build.
- Parsing treats the build suffix as present only when the part after the final
  `-` matches `yurt_<number>`. Otherwise the full string after `@` is parsed as
  SemVer, so pre-release versions such as `1.0.0-alpha.1` remain valid exact
  version pins.
- Names must pass the existing package-name validator.
- Versions must parse as SemVer.
- Build ids are opaque strings for storage, but Yurt build ordering accepts
  only `yurt_<number>` for package selection in v1. Invalid build ids in
  repository metadata are ignored as install candidates and must be rejected
  by publish CI.

## Resolver Policy

The resolver operates over cached package metadata only. It does not fetch
remote metadata during solve; users run `pkg update` first.

For each package name, candidate versions come from trusted repos. Repository
priority is applied at package selection time:

1. Lower numeric repo priority wins.
2. Ties break by lexical repo id.
3. Within a repo, highest non-yanked version wins unless a spec or dependency
   requirement narrows it.
4. For the same upstream version, highest build wins.

Yanked versions are skipped. v1 deliberately does not install yanked versions,
even when the user pins an exact version. If an exact pin targets a yanked
version, the resolver rejects it and names the yanked reason when present.
The forward path is an explicit `--allow-yanked` flag that applies only to
exact version/build pins.

Installed packages participate as fixed constraints:

- If an installed package satisfies every new constraint, keep it installed and
  do not include it in the install transaction.
- If a requested install would require changing an installed package version or
  build, fail before downloads. The diagnostic must name the installed version
  and the conflicting requirement.
- `pkg install` may install missing dependencies.
- `pkg install` must not auto-upgrade already-installed packages. Changing
  installed versions belongs to `pkg upgrade`.

This is deliberately conservative. It matches the common split between
installing missing packages and upgrading already installed packages, while
keeping the first resolver deterministic.

## Dependency Resolution Algorithm

v1 uses a complete but simple backtracking resolver.

Inputs:

- requested specs;
- trusted repo priority map;
- package metadata loaded from current cache snapshots;
- installed package records;
- yanked policy.

Output:

- selected package versions and source repos for every package that must be
  installed;
- packages already installed and reused;
- dependency graph for install ordering.

Algorithm:

1. Convert each requested spec into an initial requirement.
2. Maintain a map from package name to accumulated SemVer requirements and
   optional exact build pin.
3. Maintain a chronological decision stack. Each stack frame records:
   - package name;
   - candidate list in preference order;
   - selected candidate index;
   - dependencies added by that selection.
4. Select the next unresolved package deterministically: the lexically first
   package name with accumulated requirements and no current selection.
5. If the package is installed:
   - check whether the installed version/build satisfies the accumulated
     requirements;
   - union dependencies recorded in `installed.sqlite` into the requirement map;
   - if later requirements from the new request graph conflict with this fixed
     installed package or any fixed installed dependency, fail with an
     installed-version conflict;
   - otherwise mark it reused and do not push a decision frame.
6. If the package is not installed, enumerate candidates in preferred order.
7. Pick the first candidate whose version/build satisfies all current
   requirements, push a decision frame, and union that candidate's dependencies
   into the requirement map.
8. If dependency union invalidates an earlier selected candidate, chronologically
   backtrack: pop decision frames until a frame has an untried candidate, remove
   requirements introduced by popped frames, and resume from the next candidate.
   This is what makes the resolver complete for the finite cached candidate
   set; a later dependency can unstick an earlier package choice.
9. If no candidate works after unwinding the stack, fail with a dependency
   conflict explaining the package name and accumulated requirements.

Dependency cycles are allowed only if all packages in the cycle resolve to
consistent versions. Install ordering collapses strongly connected components
and then orders components topologically. Cycles are installed within the
component in lexical package-name order. v1 has no install-time package hooks;
if hooks are added later, packages in dependency cycles must either have no
hooks or the installer must reject the cycle before applying files.

## Install Ordering

The install transaction includes only packages not already installed at the
selected version/build.

Order:

1. Build a graph from selected package to selected dependencies.
2. Exclude reused installed dependencies from the transaction order.
3. Topologically sort dependencies before dependents.
4. Collapse dependency cycles into strongly connected components and order
   members lexically inside each component.

This follows the standard package-manager rule that dependencies are installed
before packages that require them.

## Archive Download And Verification

The installer must download and verify all selected archives before any
filesystem mutation.

For each selected package version:

1. Resolve `PackageVersion.url` against the trusted repository base URL if it
   is relative.
2. Fetch the archive bytes.
3. Check byte size and SHA-256 against repository metadata.
4. Verify the package signature bundle against the per-version signing subject
   and issuer. Until real Sigstore archive verification is wired, local
   file-backed test installs use the same static verifier boundary already used
   by `pkg update`; production installs must keep failing closed rather than
   accepting unsigned archives.
5. Parse the archive with `yurt_pkg_format::Reader`.
6. Require archive `info/index.json` name/version/build/platform/dependencies
   to match the selected repository metadata. Dependency equality is semantic:
   compare parsed dependency names and parsed SemVer requirements, not JSON byte
   order. Publish CI must generate repository metadata from the package
   archive, so production mismatches are repository corruption.
7. Refuse archives containing invalid paths, duplicate entries, or manifest
   mismatches. The archive reader already enforces these facts.

If any archive fails verification, the transaction aborts before filesystem
mutation.

## Installed State

Installed state lives under:

```text
/var/lib/yurt-pkg/installed.sqlite
```

Installed paths stored in the database are canonical package-relative paths
rooted at the sandbox root. They must be normalized exactly like archive entry
paths: no leading slash, no empty components, no `.` or `..`, and UTF-8 only.
The installer applies them under `/`, unless tests pass a hidden `--root`
override.

All mutating installed-state commands take an exclusive advisory lock before
opening `installed.sqlite` or inspecting destination paths:

```text
/var/lib/yurt-pkg/.lock
```

The lock covers resolution against installed packages, collision checks,
staging, database writes, and filesystem application. This prevents concurrent
`pkg install` invocations from racing between "path is available" and "path was
written".

Schema version 1:

```sql
PRAGMA user_version = 1;

CREATE TABLE transactions (
  id TEXT PRIMARY KEY,
  state TEXT NOT NULL CHECK (state IN ('prepared', 'committed', 'failed')),
  created_at TEXT NOT NULL,
  committed_at TEXT,
  error TEXT
);

CREATE TABLE packages (
  name TEXT PRIMARY KEY,
  version TEXT NOT NULL,
  build TEXT NOT NULL,
  repo_id TEXT NOT NULL,
  source_url TEXT NOT NULL,
  sha256 TEXT NOT NULL,
  size INTEGER NOT NULL,
  installed_at TEXT NOT NULL,
  install_transaction_id TEXT NOT NULL,
  install_state TEXT NOT NULL CHECK (install_state IN ('prepared', 'installed')),
  index_json TEXT NOT NULL,
  files_json TEXT NOT NULL,
  yurt_json TEXT,
  FOREIGN KEY (install_transaction_id) REFERENCES transactions(id)
);

CREATE TABLE dependencies (
  package_name TEXT NOT NULL,
  dependency_name TEXT NOT NULL,
  requirement TEXT NOT NULL,
  PRIMARY KEY (package_name, dependency_name)
);

CREATE TABLE files (
  path TEXT PRIMARY KEY,
  package_name TEXT NOT NULL,
  install_transaction_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  sha256 TEXT,
  target TEXT,
  mode TEXT NOT NULL,
  uid INTEGER NOT NULL,
  gid INTEGER NOT NULL,
  FOREIGN KEY (package_name) REFERENCES packages(name),
  FOREIGN KEY (install_transaction_id) REFERENCES transactions(id)
);

CREATE INDEX transactions_state_idx ON transactions(state);
CREATE INDEX packages_install_transaction_idx
  ON packages(install_transaction_id);
CREATE INDEX files_install_transaction_idx
  ON files(install_transaction_id);
```

The database stores the exact installed manifests for rows whose
`install_state = 'installed'` so later commands can:

- list installed packages;
- check installed versions during resolution;
- detect file ownership conflicts;
- uninstall packages without rereading historical archives;
- audit what was installed even if repository metadata changes.

The `files` table stores file, symlink, and hardlink ownership. It does not
store directory entries. Package directory entries are preserved inside
`files_json`; the installer creates directories as needed while applying
payloads. This keeps shared directory paths representable without needing a
many-owner path table in v1. The `files` table is a derived index of
`files_json` entries whose kind is file, symlink, or hardlink, and the two must
be updated in the same database transaction.

`pkg list` reports only packages with `install_state = 'installed'`. Prepared
rows are visible only to recovery. Database writes are transactional. A failure
before creating a prepared transaction leaves `installed.sqlite` unchanged.

## File Ownership And Collision Rules

Before mutation, compare every payload entry in the transaction against:

- existing installed `files`;
- other files in the same transaction.

Rules:

- Two different packages must not own the same path.
- A package reinstalling the exact same name/version/build/repo_id/source
  SHA-256 is a no-op in v1. If name/version/build match but the recorded
  SHA-256 differs, the installer reports a republish mismatch and refuses the
  install.
- Directories may be shared if all packages agree the path is a directory and
  mode/uid/gid match. Directory entries are not recorded in `files`; they remain
  in each package's stored `files_json` manifest. Parent directories synthesized
  only to create a child path are likewise not recorded as package-owned paths.
- File, symlink, and hardlink paths must not collide with directory paths
  declared in any installed or in-transaction `files_json`.
- File, symlink, and hardlink path collisions across different packages are
  errors, even if bytes or targets match.
- A package may not overwrite an unmanaged existing filesystem path in v1. The
  installer reports the path and package rather than taking ownership of
  unknown files.

## Filesystem Application

v1 install atomicity is database-first plus conservative file writes:

1. Take `/var/lib/yurt-pkg/.lock`.
2. Run recovery for any prepared transactions.
3. Resolve the transaction.
4. Download and verify archives.
5. Check file collisions and unmanaged destination paths while holding the
   install lock.
6. Write payload entries into a staging directory under
   `/var/lib/yurt-pkg/staging/<transaction-id>/root`.
7. In one SQLite transaction:
   - insert a `transactions` row with `state = 'prepared'`;
   - insert package rows with `install_state = 'prepared'`;
   - insert dependency and file rows for the transaction.
8. Copy staged entries into the sandbox root in dependency order.
9. In one SQLite transaction, set package rows to `install_state = 'installed'`
   and the transaction row to `state = 'committed'`.
10. Remove staging and release the install lock.

Recovery is invoked at the start of every mutating installed-state command:
`install`, `upgrade`, and `remove`. v1 implements recovery for prepared
install transactions by validating staged files still exist under
`/var/lib/yurt-pkg/staging/<transaction-id>/root` and then either completing the
copy plus commit mark, or removing the transaction's prepared `files`,
`dependencies`, and `packages` rows and marking the transaction `failed` in one
SQLite transaction if staging is missing or corrupt. Failed transactions retain
only the `transactions` audit row, so their package and path reservations do
not block a retry.

The recovery copy step is idempotent. For each staged file, symlink, or
hardlink, recovery skips a destination that already matches the staged entry's
contents or target plus mode/uid/gid. For hardlink entries, match additionally
requires the destination to share an inode with the staged target. Otherwise, it
overwrites the destination with the staged entry while holding the install lock,
then commits the transaction. `pkg list` ignores prepared rows, so users do not
see half-installed packages as installed.

If a failure happens before step 7, no visible package state changes. If a
failure happens after step 7, the next mutating package command runs recovery
before doing new work. True whole-root atomicity requires image-generation
support and is deferred.

The implementation keeps the apply boundary narrow so an image builder can
replace staging/copy/commit with an atomic generation switch, similar to Nix
profiles.

## Cache Freshness For Install

`pkg install` uses the same freshness rule as cache reads:

- If a current repo snapshot is past `expires_at` but still within the update
  flow's grace period, `pkg install` warns and continues.
- If a current repo snapshot is past `expires_at + grace`, `pkg install`
  refuses to resolve from that repo and tells the user to run `pkg update`.
- A repo with no current snapshot is ignored for solving after printing
  `repo <id> has no cache; run pkg update`.

This keeps installs offline-capable while refusing very stale signed metadata.

## CLI Behavior

`pkg install`:

- accepts one or more package specs;
- enforces the cache freshness rules above before resolving;
- prints the selected install plan before applying it;
- exits nonzero without mutation on resolver, download, verification, or
  collision errors;
- records installed state on success.

`pkg list`:

- reads `installed.sqlite`;
- prints installed package name, version, build, and repo id;
- ignores `--yanked` in v1 because yanked-installed reporting requires joining
  installed state against current cache metadata.

`pkg upgrade`:

- uses the same resolver but allows selected installed packages to change;
- is not required for the first install slice.

`pkg remove`:

- depends on installed file ownership and reverse-dependency checks;
- is not required for the first install slice.

## Error Messages

Diagnostics name the package, version/build, and requirement whenever
possible:

- `package busybox not found in local cache; run pkg update`
- `installed zlib 1.2.12-yurt_0 conflicts with required zlib ^1.3 from foo`
- `no candidate for foo satisfies >=2.0,<3.0`
- `foo 1.0.0-yurt_0 would overwrite path bin/sh owned by busybox`
- `archive hash mismatch for foo 1.0.0-yurt_0`
- `prepared install transaction tx-20260509-1 is incomplete; completed recovery`
- `prepared install transaction tx-20260509-2 is incomplete and staging is missing; marked failed`

## Tests And Acceptance

Unit tests:

- spec parser supports name, exact version, and exact version/build;
- resolver chooses latest non-yanked version for plain names;
- exact version pins choose highest build;
- installed packages are reused when compatible;
- installed version conflicts fail;
- transitive dependencies are ordered before dependents;
- cycles resolve only when versions are consistent;
- yanked versions are skipped;
- repository priority affects source selection deterministically.

Integration tests:

- `pkg update` a file-backed repository, then `pkg install app`;
- install writes files, symlinks, and `installed.sqlite`;
- second install of the same package is a no-op;
- file collision aborts before filesystem mutation;
- archive hash mismatch aborts before filesystem mutation;
- `pkg list` shows installed packages.

End-to-end acceptance:

1. Publish a package with at least one dependency.
2. Run `pkg update`.
3. Run `pkg install <package>`.
4. Confirm dependency packages are installed first.
5. Confirm installed manifests are recorded.
6. Run an installed command from the sandbox root.
