# Yurt Package Distribution & Trust Design

**Status:** Draft
**Date:** 2026-05-07
**Scope:** Signing, repository format, trust model, publish CI, dependency
declaration, the install/upgrade contract, and the in-sandbox `pkg`
binary's command surface. Resolver algorithm internals are deferred to a
follow-up spec.

## Summary

Today `yurt-pkg` defines a package archive format and a host-side builder
(`yurt-pack`). It has no signing, no repository, no notion of trust, and no
in-sandbox client. This spec adds:

1. A signature scheme for individual packages, using sigstore-keyless via
   GitHub Actions OIDC, with cosign-style sidecar files.
2. A signed repository manifest hosted in a GitHub "repository repository",
   organised so PRs to add a package version touch one file.
3. A trust model rooted at image build time: the official repo's signing
   identity is baked into the image's `trusted-repos.toml`. Per-package
   continuity ("same signer as last version") is enforced by the repository
   repo's own CI, not by clients.
4. A recipe format that lets the repository repo's CI build, sign, and
   publish each package on the contributor's behalf in v1.
5. The in-sandbox `pkg` binary's command surface (update / search / info /
   install / upgrade / remove / list / add-repo / trust), with concrete
   filesystem and capability requirements.
6. A dependency declaration format (cargo/npm-style SemVer requirements) and
   the user-visible install/upgrade contract: transitive install, collision
   detection across the closure of currently-installed and newly requested
   packages, all-or-nothing application. The resolver algorithm itself is
   deferred to a follow-up spec.

## Goals

- Downloads are verifiable: the client can prove a package was signed by the
  identity the repo says it should be signed by, before installation.
- A new version of an existing package cannot be published under a different
  signing identity without an explicit, reviewable change in the repository
  repo.
- `pkg update` is cheap: a 304 round-trip when nothing changed; only changed
  per-package files re-fetched otherwise.
- Trust changes (adding a repo, accepting a new identity) are privileged and
  cannot be made by code running in the default sandbox.
- The data model is federation-ready: any package's listing can later point
  at an externally-hosted artifact without changing the index format.
- Installing a package transitively installs missing declared dependencies.
  `pkg install` does not silently change already-installed package versions; if
  a requested package would require such a change, it reports the conflict and
  asks the user to run `pkg upgrade`. `pkg upgrade` owns version changes for
  installed packages. Version collisions across the resolved set are detected
  and reported before any filesystem mutation.

## Non-goals

- Resolver algorithm details (constraint solving strategy, backtracking
  behaviour, conflict-resolution heuristics, install hooks). The user-visible
  contract — transitive install, version-collision detection, all-or-nothing
  application — is in scope here; the algorithm is its own spec.
- Automated supply-chain review (LLM diffing, malware scanners). Deferred.
- True revocation lists. v1 supports yank only; key rotation goes through
  image-build-time trust changes.
- Federated authoring in v1. The data model supports it; the CI does not.
- Bit-reproducible builds. Recipes pin source hashes but not toolchain hashes.

## Architecture

### Package artifact

A published package is a sigstore-signed `.yurtpkg` file with one sidecar
artifact, both attached to a GitHub Release on the repository repo:

```
foo-1.0.0.yurtpkg                     # the existing zstd-tar archive,
                                      # renamed (no .tar.zst suffix; the
                                      # internal magic remains the source
                                      # of truth for format detection)
foo-1.0.0.yurtpkg.bundle              # Sigstore Bundle (single JSON):
                                      #   signature + Fulcio cert chain +
                                      #   Rekor inclusion proof
```

The bundle is produced by `cosign sign-blob --yes --bundle <out>.bundle
foo-1.0.0.yurtpkg`. It is the [Sigstore Bundle Format][bundle] — one file
that carries the signature, the Fulcio-issued certificate (with its OIDC
subject and issuer extensions), and the Rekor inclusion proof with its
`integratedTime`. Verification is offline-capable given the Fulcio root
CA and Rekor public key bundled with the client.

The signature commits to the archive bytes, which transitively commit to
every file's content via the existing `info/files.json` per-entry hashes.
The Rekor `integratedTime` provides a signed lower bound on when the
artifact was published; clients use it for freshness checks (see
`pkg update` flow).

[bundle]: https://github.com/sigstore/protobuf-specs/blob/main/protos/sigstore_bundle.proto

### Repository manifest

The repository repo is a single GitHub repo (e.g.
`github.com/YurtOS/yurt-packages`) laid out as:

```
index.json                          # signed top-level index
index.json.bundle                   # Sigstore Bundle covering index.json
packages/
  foo.json                          # all versions of foo
  bar.json
recipes/
  foo/
    1.0.0/
      recipe.toml
      build.sh                      # only when extended_build = true
  bar/
    ...
```

`index.json` is the only signed object the client trusts directly. It
commits to the hash of every `packages/<name>.json`, so the per-package
files are transitively trusted without their own signatures. It also
carries monotonic-version and expiry metadata for rollback and
freshness protection:

```json
{
  "schema": 1,
  "index_version": 4271,
  "generated_at": "2026-05-07T12:00:00Z",
  "expires_at":   "2026-05-14T12:00:00Z",
  "packages": {
    "foo": {
      "sha256": "<hash of packages/foo.json>",
      "size": 1234,
      "url": "packages/foo.json"
    },
    "bar": { ... }
  }
}
```

`index_version` is a strictly increasing integer bumped by CI on each
regeneration. Clients refuse any `index.json` whose `index_version` is
not greater than the cached one — this defends against rollback to an
older index that hides a yank or pins a vulnerable version.

`expires_at` is a hard expiry: clients treat the cached index as stale
past this point and warn loudly, refusing to use it for installs after
a configurable grace period (default 30 days past expiry). The CI
regenerates and re-signs the index on every package change *and* on a
weekly cron regardless, so under normal operation `expires_at` is
always in the future.

Together these mirror TUF's freshness and rollback rules without
adopting the full TUF metadata stack.

`packages/<name>.json` lists every version of that package:

```json
{
  "name": "foo",
  "versions": [
    {
      "version": "1.0.0",
      "build": "yurt_0",
      "url": "https://github.com/YurtOS/yurt-packages/releases/download/foo-1.0.0/foo-1.0.0.yurtpkg",
      "sha256": "<archive sha256>",
      "size": 56789,
      "signing": {
        "subject": "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main",
        "issuer":  "https://token.actions.githubusercontent.com"
      },
      "yanked": false
    },
    {
      "version": "1.0.1",
      ...
      "yanked": true,
      "yanked_reason": "CVE-2026-1234, upgrade to 1.0.2"
    }
  ]
}
```

The `url` field is the public abstraction. In v1 every URL resolves to a
GitHub Release on the repository repo itself. A future federated mode
points the URL elsewhere; the index format does not change.

The `signing` block pins both the OIDC **subject** (the workload identity,
e.g. a GitHub Actions workflow ref) and the OIDC **issuer** (the identity
provider, e.g. `https://token.actions.githubusercontent.com`). Sigstore
keyless verification requires both — pinning only the subject would let a
cert from a different OIDC provider that happened to mint a token with the
same subject string pass verification. In v1 every package's signing
identity is the repository repo's own release workflow; in a federated
future, it varies per package.

### Trust model

Trust is rooted in two files written at image build time and read-only at
runtime:

```toml
# /etc/yurt-pkg/trusted-repos.toml
[[repo]]
id              = "yurt-core"
url             = "https://github.com/YurtOS/yurt-packages"
signing_subject = "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main"
signing_issuer  = "https://token.actions.githubusercontent.com"
priority        = 0
```

The image builder (yurt's existing build process) bakes in the official
repo entry. A builder flag overrides this with a custom core repo or
omits the file entirely. There is no first-run TOFU prompt in the
sandbox: trust decisions happen at build time where the operator is
already in a high-trust context.

Adding or modifying repos at runtime requires a `repo:write` capability
that the default sandbox does not grant. The same code path runs at boot
or rebuild time when running with elevated privileges; the sandbox UX is
"command exists, returns a permission error".

Per-package signing identity is **not** stored on the client. The signed
`index.json` declares each package's expected `signing.subject` and
`signing.issuer` via the per-version entries; the client verifies the
artifact's sigstore cert against both fields at install time. The "same
identity as the last version" rule is enforced by the repository repo's
CI on PRs (see Repository repo CI flow). A legitimate identity migration
is a reviewable PR diff, not a client prompt.

### Client filesystem layout

```
/etc/yurt-pkg/
  trusted-repos.toml                  # immutable from default sandbox
  sigstore-trust-root/                # Fulcio root CA + Rekor public key,
                                      #   pinned at image build. Updated
                                      #   only via image rebuild. Format
                                      #   matches the Sigstore TUF
                                      #   trust-root layout.

/var/cache/yurt-pkg/repos/<repo-id>/  # writable runtime state
  index.json
  index.json.bundle                   # Sigstore Bundle (sig + cert + Rekor proof)
  meta.json                           # etag, last_fetched, last_index_version,
                                      #   last_integrated_time,
                                      #   consecutive_fetch_failures
  packages/
    foo.json
    bar.json
  db.sqlite                           # search index: name, summary,
                                      #   latest non-yanked version, etc.

/var/lib/yurt-pkg/
  installed.sqlite                    # what is installed, owned files,
                                      #   versions, install identities
```

`<repo-id>` is the user-chosen `id` from `trusted-repos.toml` (e.g.
`yurt-core`), not a hash — friendlier in error messages, already
namespaced by the trusted config.

### `pkg update` flow

1. `GET <repo url>/raw/main/index.json` with
   `If-None-Match: <cached-etag>`. On 304: re-evaluate the cached index's
   `expires_at` against the current time. If still in the freshness
   window, write a fresh `last_fetched` and exit successfully. If past
   `expires_at`, treat the same as a stale-cache failure: bump
   `consecutive_fetch_failures`, emit the escalating staleness warning,
   and — past the configured grace period — make subsequent install /
   upgrade operations refuse to run until a non-304 update succeeds.
   The 304 path must not silently extend the freshness window just
   because the upstream bytes are unchanged; freshness is a property of
   the signed index, not of the HTTP response.
2. On 200: download `index.json` and `index.json.bundle`. Verify, in
   order, all of:
   - The bundle's Fulcio cert chains to the trusted Fulcio root.
   - The cert's OIDC subject equals the repo's `signing_subject`, and the
     cert's OIDC issuer extension equals the repo's `signing_issuer`.
   - The signature in the bundle is valid over `sha256(index.json)` using
     the cert's public key.
   - The Rekor inclusion proof in the bundle verifies against the trusted
     Rekor public key.
   - The new `index_version` is strictly greater than the cached
     `last_index_version` (rollback check).
   - The Rekor `integratedTime` is greater than or equal to the cached
     `last_integrated_time` (independent freshness check; defends against
     replay even when `index_version` is suppressed).
   - `expires_at` is in the future, modulo a configurable past-grace.

   Any failure aborts the update and leaves the cache untouched. The
   client continues to serve queries from the last-good index;
   `consecutive_fetch_failures` is bumped so `pkg update` can warn after N
   failures, and `expires_at` lateness is its own escalating warning.
3. Parse the new index. Diff its `packages` map against the cached one. For
   every package whose `sha256` changed (or is new), fetch
   `packages/<name>.json` and verify its hash against the value committed
   by the signed index. No separate per-package signature.
4. Update `db.sqlite` incrementally for changed packages only. Drop entries
   for removed packages. Persist the new `index_version` and `integratedTime`
   to `meta.json`.

### `pkg install` and `pkg upgrade` verification

Per package being installed:

1. Look up `<name>` in `db.sqlite`, find the requested version (or latest
   non-yanked).
2. Fetch the `url` to a temp file. Stream-hash to verify against the
   `sha256` committed by the (already-trusted) index.
3. Fetch `url + ".bundle"`. Verify, in order:
   - Fulcio cert chains to the trusted Fulcio root.
   - Cert's OIDC subject equals the version's `signing.subject`, issuer
     equals `signing.issuer`.
   - Signature is valid over the archive's bytes.
   - Rekor inclusion proof verifies; `integratedTime` is consistent with
     the cert's `notBefore`/`notAfter` window.
4. Hand the archive to `yurt-pkg-format` for the existing per-file
   validation against `info/files.json`.
5. Stage filesystem writes and commit installed package state in
   `installed.sqlite` atomically. The v1 all-or-nothing contract is package
   database visibility: failed or prepared transactions are not reported as
   installed packages to subsequent commands. Filesystem writes are repairable
   rather than whole-root-atomic until stronger image-generation or snapshot
   support exists; the resolver/installer spec owns that recovery strategy.

Yanked versions never appear in resolver results. An explicit
`pkg install foo@1.0.1` of a yanked version fails with the
`yanked_reason` printed; `--allow-yanked` is the override.

### Recipe format

A recipe lives at `recipes/<name>/<version>/recipe.toml`:

```toml
# recipes/foo/1.0.0/recipe.toml
[source]
url     = "https://example.org/foo-1.0.0.tar.gz"
sha256  = "abc123..."

[build]
# CI runs these in order in a clean container, with $STAGE pointing at
# the staged install root that yurt-pack will turn into a package.
steps = [
  "tar xzf $SOURCE -C $BUILD_DIR",
  "cd $BUILD_DIR/foo-1.0.0 && ./configure --prefix=/usr",
  "cd $BUILD_DIR/foo-1.0.0 && make",
  "cd $BUILD_DIR/foo-1.0.0 && make DESTDIR=$STAGE install",
]

# Set extended_build = true to add a build.sh next to recipe.toml that CI
# will execute instead of `steps`. Reviewers should treat this as a flag
# warranting more scrutiny — it is arbitrary code running with the
# repository repo's signing identity.
extended_build = false

[package]
# Fields that flow into yurt-pack.toml / info/index.json.
name        = "foo"
version     = "1.0.0"
build       = "yurt_0"
platform    = "wasm32-wasip1-yurt"
summary     = "Foo, a thing"
license     = "MIT"
default_uid = 0
default_gid = 0

# Dependencies use cargo/npm-style SemVer requirement strings. The recipe
# form is a name → requirement table for ergonomics; archives and the
# repo index normalise this to a JSON array (see Dependency declaration
# below).
[package.depends]
libfoo = "^1.2"            # >=1.2.0, <2.0.0
libbar = ">=0.5, <1.0"     # explicit range
libbaz = "~1.4.0"          # >=1.4.0, <1.5.0

[package.yurt]
min_yurt_version = "0.1.0"

# The signing identity this version will be published under. CI compares
# this against the most recent existing version's `signing` block in
# packages/<name>.json before merge (signer-continuity check). For v1,
# the only legal value is the repository repo's own release workflow;
# the lint rejects anything else. The block is declared in the recipe
# rather than synthesised so that continuity can be enforced at PR time,
# before any signed JSON is regenerated.
[package.signing]
subject = "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main"
issuer  = "https://token.actions.githubusercontent.com"
```

Most packages use the declarative `steps` form. The `extended_build`
escape hatch handles the long tail without making every recipe an
arbitrary-code-execution review.

### Dependency declaration

A dependency is a `{ name, req }` pair, where `req` is a cargo/npm-style
SemVer requirement string (`^1.2`, `~1.4.0`, `>=0.5, <1.0`, `*`, etc.) as
parsed by the `semver` crate's `VersionReq`. The same shape is carried in
three places:

- `recipe.toml` `[package.depends]` — a `name → req` table for ergonomic
  authoring (see the recipe example above).
- `info/index.json` inside the built archive — `yurt-pack` normalises the
  recipe table into a JSON array of `{ name, req }` objects. The existing
  `depends` field's schema is bumped from `[string]` to that array shape;
  `yurt-pkg-format` carries the migration.
- `packages/<name>.json` per-version entries — denormalised into the index
  in the same array form, so the resolver can plan a transaction without
  downloading archives.

Versions in releases are SemVer 2.0.0. A non-SemVer version or an
unparseable `req` is a recipe lint error in CI.

The user-visible contract for `pkg install` and `pkg upgrade`:

1. Build the dependency closure of the requested set **unioned with the
   set of already-installed packages**. Already-installed packages
   contribute their declared `depends` constraints to the solve, so the
   resolver cannot pick a version of some shared library that would
   violate an existing installed package's requirements.
2. For each name in the closure, pick the highest non-yanked version
   satisfying *every* constraint placed on it across the closure (from
   newly requested packages, from their transitive deps, and from
   already-installed packages).
3. If no version satisfies all constraints for some name, abort the
   transaction with a "version collision" error naming the conflicting
   constraints and which packages declared them. No filesystem
   mutation has happened yet.
4. Otherwise, compute the diff against currently-installed versions. For
   `pkg install`, the diff may add missing packages but must not change an
   already-installed package version/build. For `pkg upgrade`, the diff may
   upgrade selected installed packages and their dependencies. Present the plan,
   and on confirmation fetch and verify every selected package (per the install
   verification flow above) and commit installed state atomically.

For v1, "atomically" means package database visibility is all-or-nothing:
commands that read installed state do not observe prepared or failed package
rows. Filesystem application is staged and repairable rather than
whole-root-atomic. A crash after some files are copied may leave partial files
in the sandbox root until the next mutating package command runs recovery, but
those files are not reported as an installed package until recovery commits the
transaction.

`pkg install` of a package whose deps would force a shared library version
change is rejected before downloads. The error names the installed version and
the requirement that would force the change. The user-facing path for that
change is `pkg upgrade <package>` or `pkg upgrade <shared-library>`.

The constraint-solving strategy itself (backtracking order, preference
between minor and patch upgrades, etc.) is the resolver spec's problem.
This spec only fixes the surface contract.

### Repository repo CI flow

On a PR that adds or modifies `recipes/<name>/<version>/`:

1. **Pre-flight**: lint the recipe; fail PRs that change `extended_build`
   from `false` to `true` without a CODEOWNERS-required review path
   (configured at the repo level, outside this spec).
2. **Signer continuity**: if `<name>` already has versions in
   `packages/<name>.json`, read the most recent existing version's
   `signing.subject` and `signing.issuer`. Compare against the proposed
   `[package.signing]` block in the new recipe. Fail the PR on any
   mismatch unless the PR also includes a `MIGRATION.md` and is approved
   by a repo maintainer. This is the "same signer as before" rule, and
   it works at PR time because the proposed identity is declared in the
   recipe — no signed JSON has been regenerated yet.
   For v1, an additional lint pins `signing.subject` and `signing.issuer`
   to the repository repo's own release workflow values; any other value
   is a hard error.
3. **Build**: in a clean container, fetch `[source].url`, verify
   `[source].sha256`, run `[build].steps` (or `build.sh`), and call
   `yurt-pack build $STAGE --manifest <derived> --out dist/`.
4. **Sign**:
   ```
   cosign sign-blob --yes \
     --bundle dist/<name>-<version>.yurtpkg.bundle \
     dist/<name>-<version>.yurtpkg
   ```
   Sigstore-keyless via the workflow's OIDC token. The single `.bundle`
   sidecar carries the signature, the Fulcio cert chain, and the Rekor
   inclusion proof — this is the artifact layout the client fetches.
5. **Release**: create a GitHub Release tagged `<name>-<version>`,
   attach `<name>-<version>.yurtpkg` and `<name>-<version>.yurtpkg.bundle`.
6. **Index regen**: regenerate `packages/<name>.json` (copying the
   recipe's `[package.signing]` into the version entry) and `index.json`
   (with `index_version` bumped and `expires_at` set to now + 7 days).
   Sign `index.json` the same way, producing `index.json.bundle`. Commit
   the three updated/signed files back to the repo's main branch.

Step 6 produces the index updates as a follow-up commit on main rather
than as part of the PR, so PRs only diff recipes and metadata, not signed
JSON. The commit is made by a bot identity tied to the same workflow.

A separate weekly cron workflow on the repository repo regenerates and
re-signs `index.json` even when no packages have changed, bumping
`index_version` and refreshing `expires_at`. This keeps the freshness
window valid for clients that haven't seen a package change in a while.

### Crate structure

```
crates/
  yurt-pkg-format       (existing)  read/write/validate the .yurtpkg archive
  yurt-pkg-repo         (new)       index.json + packages/<name>.json read/write,
                                    diffing for `pkg update`, sigstore verification glue
  yurt-pkg-trust        (new)       trusted-repos.toml, sigstore trust-root
                                    loading, signing-identity policy
                                    (subject + issuer matching against the
                                    trusted entries; no per-package keys)
  yurt-pack             (existing)  local builder; gains an ad-hoc `--sign` flag.
                                    CI uses cosign directly and does not depend on this.
  yurt-repo-ci          (new)       run by the repository repo's GH Actions: parse
                                    recipe → build in container → call yurt-pack →
                                    regenerate signed index.json. May start as a
                                    shell script and promote to Rust.
  pkg                   (new)       in-sandbox binary: update / search / info /
                                    install / upgrade / remove / list / add-repo /
                                    trust. Internals deferred to follow-up spec.
```

**Sigstore on WASI risk**: the whole plan rides on the `sigstore` Rust
crate (rustls-based) working under `wasm32-wasip1`. Smoke-test this
before sinking work into `yurt-pkg-repo`.

If the `sigstore` crate does not work cleanly under WASI, the fallback
is to reimplement only the verification path the client needs (signing
remains in CI via `cosign`, which runs on the host):

- Bundle parsing: parse the Sigstore Bundle protobuf/JSON ourselves.
- Cert-chain validation: use `webpki` (rustls's X.509 verifier) to
  validate the embedded Fulcio cert chain against a Fulcio root pinned
  in the client. Fulcio's root is the issuing CA; the per-signature
  cert is ephemeral and not trusted by itself.
- OIDC subject/issuer extraction: parse the X.509 SAN/extensions for
  the Sigstore-defined OIDs (`1.3.6.1.4.1.57264.1.x`) and compare
  against the trusted `signing_subject` / `signing_issuer`.
- Signature: ECDSA-P256 verify of the cert's public key over the
  hashed payload.
- Rekor: parse the inclusion proof in the bundle, verify the SET
  against the pinned Rekor public key.

The point is that the trust roots are still the *Fulcio CA* and the
*Rekor public key* — both pinned in the client at image build time
alongside `trusted-repos.toml`. We do not pin per-package public
keys; that would be wrong because Fulcio certs are ephemeral. The
data shapes (bundles, manifest entries) do not change between the
crate-based and fallback paths.

### `pkg` (in-sandbox binary) surface

| Command | Needs |
|---|---|
| `pkg update` | network, fs write to `/var/cache/yurt-pkg/` |
| `pkg search <query>` | fs read of `db.sqlite` |
| `pkg info <name>` | fs read of `db.sqlite` |
| `pkg install <name>[@<version>]` | network, fs write to install root, resolver, signature verification |
| `pkg upgrade [<name>...]` | as install. No args = upgrade all. Skips yanked. Default prompts; `-y` skips; `--dry-run` prints plan only. Hard errors on transitive downgrade unless `--allow-downgrade`. |
| `pkg remove <name>` | fs write to install root, `installed.sqlite` |
| `pkg list [--yanked]` | fs read of `installed.sqlite` |
| `pkg add-repo <url> --signing-subject <s> --signing-issuer <i> [--id <name>] [--priority <n>]` | `repo:write` capability — denied in default sandbox. Both subject and issuer are required; there is no default for either, and there is no interactive prompt. The added entry has the same shape as a `[[repo]]` block in `trusted-repos.toml`. |
| `pkg trust ...` | `repo:write` capability. Subcommands for inspecting and (with capability) modifying the trust store; concrete subcommand surface deferred. |

The all-or-nothing transaction *contract* for v1 is package database
visibility: no partial package state is visible to installed-state readers
after a failed install/upgrade. Filesystem writes are staged and recoverable,
but not whole-root-atomic until image-generation or snapshot support exists.
The *implementation strategy* for stronger filesystem atomicity (journaled
rename, snapshot-and-swap), along with resolver algorithm internals, install
hooks, and conflict handling between files owned by different packages, is out
of scope for this spec.

## Open questions

- **Index storage on main vs. release tag**: `index.json` and its sigs are
  proposed to live on the `main` branch and update via bot commits. An
  alternative is to attach the signed index to a rolling
  `index-latest` GitHub Release. The `main`-branch model gives diffable
  history; the release model isolates the signed artifact from
  unrelated commits. Default to `main`-branch unless a concrete reason
  surfaces.
- **CDN / rate-limits**: `raw.githubusercontent.com` has unauthenticated
  rate limits. For modest user counts this is fine; if it becomes a
  problem the index can be served from a GitHub Pages site or jsDelivr
  mirror without changing signatures.
- **Recipe build container choice**: not specified here. The CI workflow
  pins it; changes are repo-PR-reviewable.

## Future work (explicitly deferred)

- Resolver internals and the `pkg install` planner (its own spec).
- LLM-driven supply-chain review on release PRs.
- Federated package authoring (third-party repos publishing under their
  own OIDC identity).
- Real revocation lists / signer rotation flows beyond yank + image
  rebuild.
- Bit-reproducible builds.
