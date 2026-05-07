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
- Installing or upgrading a package transitively installs or upgrades its
  declared dependencies. Version collisions across the resolved set are
  detected and reported before any filesystem mutation.

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

A published package is a sigstore-signed `.yurtpkg` file with two sidecar
artifacts, all attached to a GitHub Release on the repository repo:

```
foo-1.0.0.yurtpkg                     # the existing zstd-tar archive,
                                      # renamed (no .tar.zst suffix; the
                                      # internal magic remains the source
                                      # of truth for format detection)
foo-1.0.0.yurtpkg.sig                 # cosign signature over sha256(archive)
foo-1.0.0.yurtpkg.cert                # sigstore cert tying signature to an
                                      # OIDC identity
```

The signature commits to the archive bytes, which transitively commit to
every file's content via the existing `info/files.json` per-entry hashes.
The Rekor transparency log entry produced by cosign is recorded but not
required for verification — clients verify against the cert's identity
fields directly.

### Repository manifest

The repository repo is a single GitHub repo (e.g.
`github.com/YurtOS/yurt-packages`) laid out as:

```
index.json                          # signed top-level index
index.json.sig
index.json.cert
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
files are transitively trusted without their own signatures:

```json
{
  "schema": 1,
  "generated_at": "2026-05-07T12:00:00Z",
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
      "identity": "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main",
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

The `identity` field is the OIDC subject the client requires the artifact's
sigstore cert to match. In v1 every package's identity is the repository
repo's own release workflow; in a federated future, it varies per package.

### Trust model

Trust is rooted in two files written at image build time and read-only at
runtime:

```toml
# /etc/yurt-pkg/trusted-repos.toml
[[repo]]
id        = "yurt-core"
url       = "https://github.com/YurtOS/yurt-packages"
identity  = "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main"
priority  = 0
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
`index.json` declares each package's expected identity via the per-version
`identity` field; the client verifies the artifact's sigstore cert against
that field at install time. The "same identity as the last version" rule
is enforced by the repository repo's CI on PRs (see Repository repo CI
flow). A
legitimate identity migration is a reviewable PR diff, not a client
prompt.

### Client filesystem layout

```
/etc/yurt-pkg/
  trusted-repos.toml                  # immutable from default sandbox

/var/cache/yurt-pkg/repos/<repo-id>/  # writable runtime state
  index.json
  index.json.sig
  index.json.cert
  meta.json                           # etag, last_fetched, last_good_cert,
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
   `If-None-Match: <cached-etag>`. On 304, write a fresh `last_fetched` to
   `meta.json` and exit successfully.
2. On 200: download `index.json`, `.sig`, `.cert`. Verify the signature's
   sigstore cert OIDC subject matches the trusted `identity` for the repo.
   Reject if not.
3. Parse the new index. Diff its `packages` map against the cached one. For
   every package whose `sha256` changed (or is new), fetch
   `packages/<name>.json` and verify its hash against the value committed
   by the signed index. No separate per-package signature.
4. Update `db.sqlite` incrementally for changed packages only. Drop entries
   for removed packages.

A failure at step 2 leaves the cache untouched; the client continues to
serve queries from the last-good index. `consecutive_fetch_failures` in
`meta.json` is bumped so `pkg update` can warn after N failures.

### `pkg install` and `pkg upgrade` verification

Per package being installed:

1. Look up `<name>` in `db.sqlite`, find the requested version (or latest
   non-yanked).
2. Fetch the `url` to a temp file. Stream-hash to verify against the
   `sha256` committed by the (already-trusted) index.
3. Fetch `url + ".sig"` and `url + ".cert"`. Verify the cert's OIDC
   subject matches the version's `identity` field. Verify the signature
   over the archive's bytes.
4. Hand the archive to `yurt-pkg-format` for the existing per-file
   validation against `info/files.json`.
5. Atomically apply to the install root and record in `installed.sqlite`.

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
4. Otherwise, compute the diff against currently-installed versions
   (installs, upgrades, downgrades, removals — though `pkg install`
   never removes, only `remove`/`upgrade` may), present it as a plan,
   and on confirmation fetch and verify every selected package (per
   the install verification flow above) and apply atomically.

`pkg install` of a package whose deps would force an upgrade of a
shared library is allowed: that upgrade appears in the plan. A forced
*downgrade* of an already-installed package is an error unless
`--allow-downgrade` is given, matching the `pkg upgrade` rule.

The constraint-solving strategy itself (backtracking order, preference
between minor and patch upgrades, etc.) is the resolver spec's problem.
This spec only fixes the surface contract.

### Repository repo CI flow

On a PR that adds or modifies `recipes/<name>/<version>/`:

1. **Pre-flight**: lint the recipe; fail PRs that change `extended_build`
   from `false` to `true` without a CODEOWNERS-required review path
   (configured at the repo level, outside this spec).
2. **Signer continuity**: if `<name>` already has versions in
   `packages/<name>.json`, fail the PR if the new version's declared
   `identity` differs from the most recent existing version's `identity`,
   unless the PR also includes a `MIGRATION.md` and is approved by a repo
   maintainer. This is the "same signer as before" rule.
3. **Build**: in a clean container, fetch `[source].url`, verify
   `[source].sha256`, run `[build].steps` (or `build.sh`), and call
   `yurt-pack build $STAGE --manifest <derived> --out dist/`.
4. **Sign**: `cosign sign-blob --yes dist/<name>-<version>.yurtpkg`.
   Sigstore-keyless via the workflow's OIDC token; `.sig` and `.cert`
   produced as sidecars.
5. **Release**: create a GitHub Release tagged `<name>-<version>`,
   attach the three artifacts.
6. **Index regen**: regenerate `packages/<name>.json` and `index.json`,
   sign `index.json` the same way, commit all three signed files back to
   the repo's main branch.

Step 6 produces the index updates as a follow-up commit on main rather
than as part of the PR, so PRs only diff recipes and metadata, not signed
JSON. The commit is made by a bot identity tied to the same workflow.

### Crate structure

```
crates/
  yurt-pkg-format       (existing)  read/write/validate the .yurtpkg archive
  yurt-pkg-repo         (new)       index.json + packages/<name>.json read/write,
                                    diffing for `pkg update`, sigstore verification glue
  yurt-pkg-trust        (new)       trusted-repos.toml, OIDC identity matching
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
before sinking work into `yurt-pkg-repo`. Fallback if it does not:
verification in CI uses `cosign` directly; the in-sandbox client uses
raw ed25519 verification against a public key extracted from the cert
and pinned in the manifest. The data shapes above do not change in
either case.

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
| `pkg add-repo <url> --identity <oidc-subject>` | `repo:write` capability — denied in default sandbox |
| `pkg trust ...` | `repo:write` capability |

Resolver behaviour, install hooks, conflict handling, and atomicity
guarantees for the install root are out of scope for this spec.

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
