# Yurt Package Update Flow Design

**Status:** Draft
**Date:** 2026-05-07
**Scope:** Executable `pkg update`, `pkg search`, and `pkg info` behavior. This
spec covers trusted repository cache refresh, local search/info indexing, and
the persistence formats those commands own. Package archive download,
dependency resolution, installation, upgrade, removal, and installed-state
mutation remain in the resolver/installer follow-up spec.

## Summary

The distribution foundation defines trusted repositories, signed repository
metadata, package metadata, and a verifier boundary. The next slice turns that
foundation into a working local repository cache:

1. `pkg update` reads trusted repo configuration from the image, fetches signed
   repository metadata, verifies the signed `index.json`, enforces freshness and
   rollback rules, downloads changed `packages/<name>.json` files, and persists
   a queryable cache.
2. `pkg search` reads that cache and returns matching package summaries without
   touching the network.
3. `pkg info` reads that cache and displays package versions, dependencies,
   repository identity, signing identity, and yanked state.

The key design choice is to make update executable while keeping network and
Sigstore details behind narrow interfaces. Tests use deterministic fetchers and
verifiers; production can wire real HTTP and real Sigstore verification without
changing cache semantics.

## Goals

- Make `pkg update` useful in a real sandbox image.
- Enforce the distribution spec's trust rules for repository metadata:
  subject + issuer matching, Fulcio/Rekor trust roots, freshness, and rollback.
- Persist enough metadata for deterministic offline `pkg search` and `pkg info`.
- Support multiple trusted repositories and deterministic repo priority.
- Support private GitHub-hosted repositories by allowing the fetch layer to
  receive authentication configuration without baking GitHub policy into cache
  logic.
- Keep install and resolver behavior out of this slice.

## Non-Goals

- Downloading `.yurtpkg` archives for installation.
- Verifying package archive bundles during install.
- Dependency solving, upgrade planning, installed package state, or filesystem
  transactions.
- Runtime trust management UX beyond reading existing `trusted-repos.toml`.
- A full TUF implementation.
- Dependency lockfile generation.

## Existing Inputs

Trust roots and repository config are image-build-time files:

```text
/etc/yurt-pkg/trusted-repos.toml
/etc/yurt-pkg/sigstore-trust-root/fulcio-root.pem
/etc/yurt-pkg/sigstore-trust-root/rekor.pub
```

Runtime cache state lives under:

```text
/var/cache/yurt-pkg/repos/<repo-id>/
  .lock
  state.json
  current -> snapshots/<snapshot-id>/
  snapshots/
    <snapshot-id>/
      index.json
      index.json.bundle
      manifest.json
      packages/
        <name>.json
      db.sqlite
  staging-<random>/
```

`<repo-id>` is the trusted repo id from `trusted-repos.toml`. The canonical
layout for this slice is the `current -> snapshots/<snapshot-id>/` layout.
`current` is a POSIX symlink and readers and writers must resolve it to find
the active immutable snapshot. `state.json` is mutable per-repo state outside
snapshots.

## Architecture

Add a cache/update layer to `yurt-pkg-repo` and wire it into the `pkg` binary.
The layer has four boundaries:

```text
pkg CLI
  -> trusted config loader
  -> repository update engine
       -> RepoFetcher trait
       -> BundleVerifier trait
       -> CacheStore filesystem persistence
       -> SearchIndex sqlite writer/reader
```

`RepoFetcher` owns bytes-on-the-wire. It exposes conditional fetches but not
package semantics:

```rust
pub struct FetchRequest<'a> {
    pub url: &'a Url,
    pub etag: Option<&'a str>,
}

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
```

The production fetcher may use HTTP and GitHub authentication. Tests use a
memory fetcher. The update engine only requires deterministic response
semantics: `NotModified` means the server claims the cached object still
matches the supplied ETag.

`BundleVerifier` is the existing verifier trait. This slice uses it for
`index.json` verification. Package archive bundle verification remains deferred
to install. Per-package JSON files are not separately signed; they are trusted
through the verified index hash.

`CacheStore` owns all filesystem writes under one repo cache directory. It
must provide the snapshot commit model described below so cache readers see one
complete repository state at a time.

`SearchIndex` owns the sqlite schema and query behavior for `search` and
`info`.

## Locking And Commit Model

Every repo cache has a lock file:

```text
/var/cache/yurt-pkg/repos/<repo-id>/.lock
```

`pkg update` takes an exclusive lock for each repo before reading or writing
that repo's cache. `pkg search` and `pkg info` take shared locks for the full
duration of resolving `current`, opening `db.sqlite`, reading `manifest.json`
and `state.json`, and querying that snapshot. If a platform cannot provide
advisory shared/exclusive file locks, the implementation must provide an
equivalent single-writer / multi-reader lock in the Yurt filesystem layer
before enabling concurrent commands.

Updates commit by writing a complete new snapshot directory and then replacing
the `current` pointer:

```text
repos/<id>/staging-<random>/
  index.json
  index.json.bundle
  manifest.json
  packages/
  db.sqlite
```

The staging directory is fsynced where the platform exposes fsync. Commit is
POSIX-only in v1: create a temporary symlink to the new snapshot and replace
`current` with a single `rename(2)`. `snapshot-id` is an opaque unique id, not
the repository `index_version`; implementations should include randomness or a
content hash so a crashed attempt cannot wedge a later retry for the same
`index_version`. Readers never open files from staging directories. Old
snapshots may be garbage-collected only while holding the exclusive repo lock
and only after they are no longer `current`.

This is the only atomicity guarantee required for v1: a reader sees either the
old complete snapshot or the new complete snapshot. There is no attempt to
atomically rename each individual metadata file into an existing directory.
Non-POSIX cache filesystems are out of scope for this slice.

## Update Flow

For each trusted repo, `pkg update` performs:

1. Take the repo's exclusive update lock.
2. Load `state.json`, resolve `current` if present, and read the current
   snapshot's `manifest.json`, `index.json`, and package file hash map.
3. Compare the current snapshot's signing subject and issuer to the current
   `trusted-repos.toml`. If either differs, treat this as the first update under
   the new signing identity: do not send old ETags, do not reuse old package
   hashes, and do not enforce rollback/Rekor monotonicity against the old
   snapshot.
4. Compare the current snapshot's repo URL to the current `trusted-repos.toml`.
   If only the URL differs, do not send old ETags and do not reuse old package
   hashes, but keep enforcing rollback/Rekor monotonicity against the old
   snapshot because the signing identity is unchanged.
5. Fetch `index.json` using the cached index ETag only when signing identity and
   repo URL both match.
6. Fetch `index.json.bundle` when the index is modified. Bundle fetch may also
   use its own ETag, but a modified index cannot be accepted with a stale or
   missing bundle.
7. If the index is `NotModified`, do not fetch the bundle. Re-validate the
   cached index's `expires_at + grace` against the current time. A stale cached
   index is a hard update error; a 304 HTTP response does not refresh signed
   metadata freshness. If this gate passes, update `state.json.last_fetched`
   under the exclusive lock; no new snapshot is produced.
8. If the index is modified, verify the new `index.json` bytes against the new
   bundle using the trusted repo's signing subject and issuer plus the local
   Sigstore trust root.
9. Parse and validate the index.
10. Enforce version rollback: the new `index_version` must be greater than the
   current snapshot's `manifest.index_version`. First update under a trust
   binding has no previous version.
11. Enforce Rekor-time replay protection: the verified bundle's
   `integrated_time` must be greater than or equal to the current snapshot's
   `manifest.integrated_time`. A strictly older Rekor time is rejected. No
   wall clock skew allowance is needed because both values come from Rekor, not
   the local system clock.
12. Enforce freshness: `now` must be no later than `expires_at + grace`.
13. Compute changed and removed package files by comparing cached package hashes
   to the verified index package map.
14. Resolve and fetch each changed package entry's signed `url`, verify its
    SHA-256 and byte size against the verified index entry, parse it as
    `PackageFile`, validate it, and require `PackageFile.name` to equal the
    index package map key.
15. Build the next complete package directory in staging. Package files removed
    from the verified index are omitted from staging, not deleted from the
    current snapshot in place.
16. Rebuild `db.sqlite` in staging from the complete staged package set.
17. Write staged `manifest.json`.
18. Commit the staged snapshot by atomically replacing `current`.
19. Update `state.json` with ETags, `last_fetched`,
    `consecutive_fetch_failures = 0`, and `current_snapshot`.

The `current` symlink plus the current snapshot's `manifest.json` are the
authoritative committed state. Rollback and Rekor-time checks use
`manifest.index_version` and `manifest.integrated_time`, not values from
`state.json`. If the process crashes after step 18 and before step 19, the new
snapshot is still committed; the next command repairs `state.json` from
`current` and `manifest.json` before continuing.

If any step fails, the previous cache state remains usable. The command exits
non-zero and reports which repo failed. A multi-repo update may continue to
other repos after one repo fails, but the final command exits non-zero if any
repo failed.

On any failure after the fetch phase begins and before a new snapshot is
committed, update increments `state.json.consecutive_fetch_failures` under the
exclusive lock and then returns the original failure. Failures before a repo
cache is initialized may report without writing `state.json`.

## Package URL Resolution

The signed index entry URL is authoritative. Clients must not assume
`packages/<name>.json` as the fetch URL.

If `RepoPackage.url` starts with `packages/`, it is a repository-relative URL
resolved against the trusted repo base URL. Existing validation already
restricts this form to one file below `packages/` ending in `.json`; path
traversal and nested relative layouts are invalid. Relative package requests
inherit the same authentication context as the index request.

If `RepoPackage.url` is absolute, the fetcher receives that absolute URL. v1
allows absolute URLs because the repository format is federation-ready, but
authentication is scoped by origin: credentials used for the trusted repo base
must not be sent to a different scheme/host/port. The package JSON is still
trusted only if its bytes match the SHA-256 and size committed by the verified
index.

All fetched package files are persisted locally as
`packages/<index-map-key>.json` inside the snapshot, regardless of their source
URL.

## Snapshot Manifest And Repo State

`manifest.json` is immutable per snapshot:

```json
{
  "schema": 1,
  "repo_id": "official",
  "repo_url": "https://github.com/YurtOS/yurt-packages",
  "signing_subject": "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main",
  "signing_issuer": "https://token.actions.githubusercontent.com",
  "index_version": 4271,
  "integrated_time": "2026-05-08T11:59:30Z",
  "expires_at": "2026-05-15T12:00:00Z"
}
```

`signing_subject` and `signing_issuer` are the security binding for the
snapshot. If either changes in `trusted-repos.toml`, `pkg search` and
`pkg info` refuse to display the snapshot with:
`trusted signing identity for repo <id> changed; run pkg update`.
`pkg update` may replace it after successfully verifying metadata under the new
identity, but it must treat this as the first update for that signing identity:
do not send old ETags, do not reuse old package hashes, and do not enforce
rollback/Rekor monotonicity against the old snapshot's
`manifest.index_version` or `manifest.integrated_time`.

`repo_url` is transport binding, not signing identity. If the repo URL changes
but signing subject and issuer are unchanged, `pkg search` and `pkg info` may
continue displaying the old snapshot while warning:
`trusted URL for repo <id> changed; run pkg update`. `pkg update` must not send
old ETags and must not reuse old package hashes across the URL change, but it
must keep enforcing rollback and Rekor monotonicity against the current
snapshot's `manifest.index_version` and `manifest.integrated_time`.

`priority` is not part of the snapshot binding. Search and info always use the
current priority from `trusted-repos.toml`, so priority-only changes require no
cache refresh and do not affect rollback state.

`state.json` is mutable per repo and lives outside snapshots:

```json
{
  "schema": 1,
  "repo_id": "official",
  "current_snapshot": "20260508T120000Z-4271-a1b2c3",
  "index_etag": "\"abc\"",
  "index_bundle_etag": "\"def\"",
  "last_fetched": "2026-05-08T12:00:00Z",
  "consecutive_fetch_failures": 0
}
```

`state.json` writes happen under the exclusive repo lock using same-directory
temporary files and `rename(2)`. `current_snapshot` is a cross-check against the
`current` symlink target. Mismatch is not corruption by itself because a crash
can occur after `current` is replaced and before `state.json` is refreshed. On
mismatch, trust `current`, read the current snapshot's `manifest.json`, and
rewrite `state.json.current_snapshot` under the exclusive lock. During this
repair, clear `index_etag` and `index_bundle_etag` unless the implementation can
prove they belong to the current snapshot; stale validators must not be sent for
bytes from a different snapshot. If `state.json` is missing or malformed but
`current` and `manifest.json` are valid, rebuild the state file with missing
ETags and preserve no failure counter. `last_fetched` is updated only after a
successful end-to-end update or a successful 304 freshness revalidation.

On failed update, `consecutive_fetch_failures` increments best-effort without
destroying the last usable cache. It is used for user-facing diagnostics only:
`pkg update` reports the current count after a failure, and `pkg search` /
`pkg info` warn when a repo has one or more consecutive update failures. It is
not used for backoff in v1.

## Search Index

`db.sqlite` is an implementation detail owned by this slice. Schema version is
stored in `PRAGMA user_version = 1`.

Minimum tables:

```sql
CREATE TABLE packages (
  repo_id TEXT NOT NULL,
  name TEXT NOT NULL,
  latest_version TEXT,
  latest_build TEXT,
  latest_yanked INTEGER NOT NULL,
  summary TEXT,
  PRIMARY KEY (repo_id, name)
);

CREATE TABLE versions (
  repo_id TEXT NOT NULL,
  name TEXT NOT NULL,
  version TEXT NOT NULL,
  build TEXT NOT NULL,
  url TEXT NOT NULL,
  sha256 TEXT NOT NULL,
  size INTEGER NOT NULL,
  signing_subject TEXT NOT NULL,
  signing_issuer TEXT NOT NULL,
  yanked INTEGER NOT NULL,
  yanked_reason TEXT,
  depends_json TEXT NOT NULL,
  PRIMARY KEY (repo_id, name, version, build)
);
```

`summary` is optional because current `packages/<name>.json` entries do not
carry package summaries. If absent, `pkg search` still lists name, selected
version, repo id, and yanked state. In v1 the column is deliberately empty for
repositories using the current package metadata schema. A later repository
schema can add summaries without changing the cache ownership model.

Latest version selection uses semantic version ordering and ignores yanked
versions by default. If multiple builds exist for the same semantic version,
the lexicographically greatest `build` value wins. This keeps selection
deterministic without assigning semantic meaning to build ids. If every version
is yanked, `latest_version` is the highest yanked version and
`latest_yanked = 1` so `pkg info` can explain that no non-yanked candidate
exists.

## `pkg search`

`pkg search <query>`:

- requires at least one readable `db.sqlite`;
- performs case-insensitive substring search over package names and available
  summaries;
- groups duplicate package names across repos using repository selection rules:
  the smallest integer priority value from the current `trusted-repos.toml`
  wins, then lexical repo id;
- displays the selected repo id and latest non-yanked version;
- exits non-zero with a clear message if no repo cache exists and suggests
  running `pkg update`.
- warns, without changing the exit code, for each trusted repo whose cache is
  missing, stale past freshness grace, signed by a now-different trusted
  identity, fetched from a now-different trusted URL, or marked with
  `consecutive_fetch_failures > 0`.

No network access occurs during search. Search is allowed to display stale
results that are still within freshness grace, but it must warn when any
trusted repo is unavailable or past freshness grace.

## `pkg info`

`pkg info <name>`:

- reads local `db.sqlite` files only;
- applies repository selection rules to choose the default repo for the package;
- prints all versions from the selected repo, newest first;
- includes build id, URL, SHA-256, size, signing subject, signing issuer,
  yanked state/reason, and dependencies;
- exits non-zero if the package is absent from all local caches and suggests
  running `pkg update`.
- accepts `--repo <id>` to show package information from a specific trusted
  repository cache instead of the selected default repo.
- warns, without changing the exit code, for missing, stale, signing-identity
  changed, URL-changed, or failing trusted repo caches just like `pkg search`.

No network access occurs during info.

## Authentication

The update engine does not know about GitHub. Production fetchers may load
credentials from environment variables or sandbox-provided secrets, for example
`GITHUB_TOKEN`, when repo URLs require private access. The fetcher must not
persist bearer tokens in `manifest.json`, `state.json`, `db.sqlite`, logs, or
errors.

HTTP errors must redact `Authorization` headers and URL-embedded credentials
before they reach `Display`, debug logs, or CLI diagnostics.

## Operational Constraints

Freshness checks depend on the sandbox's current time. A severely skewed clock
can make valid metadata appear expired or can extend the apparent life of stale
metadata. Yurt images that use `pkg update` need a trustworthy enough wall
clock for repository freshness policy. Rollback protection does not depend on
the wall clock; it uses the current snapshot's persisted `manifest.index_version`
and Rekor `manifest.integrated_time`.

The v1 cache commit model is POSIX-only. It depends on advisory locks and
atomic `rename(2)` replacement of a temporary symlink over `current`.
Non-POSIX cache backends require a separate commit primitive and are out of
scope for this slice.

## Error Handling

Important errors should be explicit:

- missing `trusted-repos.toml`;
- malformed trusted repo config;
- missing Sigstore trust root files;
- display refused after trusted signing identity change;
- display warning after trusted repo URL change;
- missing or malformed current snapshot manifest;
- unsigned or unverifiable index;
- signing subject or issuer mismatch;
- stale cached index after 304;
- index rollback;
- older Rekor integrated time than the cached snapshot;
- package JSON hash or size mismatch;
- package JSON name mismatch against the verified index key;
- invalid package metadata;
- network failure;
- cache write failure.

Update failures must not leave mixed metadata visible. A new index cannot
become `current` without matching package JSON files and a rebuilt search DB.

## Testing

Unit tests should cover:

- `manifest.json` and `state.json` serialization and schema validation;
- 304 revalidation refuses stale cached `expires_at`;
- rollback rejection against the current snapshot's `manifest.index_version`;
- modified index requires a modified, verified bundle;
- new bundle `integrated_time` must be greater than or equal to cached
  `manifest.integrated_time`;
- subject and issuer are passed to the verifier;
- package JSON hash and size checks;
- package JSON name must match the verified index key;
- absolute package URLs do not inherit credentials for a different origin;
- changed/removed package file persistence;
- failed update preserves the previous cache;
- search groups duplicate package names by current trusted repo priority and
  repo id;
- same-version build ties resolve by lexicographically greatest build id;
- info prints versions, dependencies, signing identity, and yanked state.
- concurrent update/search/info locking behavior;
- signing identity changes for the same repo id refuse old snapshots;
- URL-only changes for the same repo id warn but keep rollback state;
- priority-only changes require no cache refresh;
- `state.json.current_snapshot` repair clears stale ETags;
- `current` is committed via POSIX symlink rename.

Integration tests should run the `pkg` binary against a temporary filesystem
layout with deterministic memory/file fetchers and a static verifier. They
should not require internet access or real Sigstore.

## Deferred Work

- Real Sigstore bundle verification if the fallback verifier work is not yet
  wired.
- Package archive download and install-time bundle verification.
- Resolver and installer behavior.
- Installed package database.
- Repo trust mutation via `pkg add-repo`.
