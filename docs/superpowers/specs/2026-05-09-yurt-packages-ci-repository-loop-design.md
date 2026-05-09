# Yurt Packages CI Repository Loop Design

## Problem

`yurt-packages` is the canonical package repository consumed by `pkg update`.
It must contain repository metadata that matches the client contract, not a
scratch fixture. Packages should be added through CI, which builds a port,
publishes the package artifact, and regenerates repository metadata.

The first acceptance package is `yurt-greet`.

## Repository Roles

- `yurt-ports` owns port recipes, patches, and build scripts. It must not
  vendor full upstream package sources. A port may fetch upstream source from a
  Git URL or release archive, and may apply local patches before building.
- `yurt-packages` owns generated repository state:
  - `index.json`
  - `index.json.bundle`
  - `packages/<name>.json`
  - package artifacts, initially under `artifacts/<name>/<version>/`
- `yurt-pkg` owns the CLI and CI helper code that understands package and
  repository metadata.

## Metadata Contract

`pkg update` fetches `<repo-url>/index.json` and `<repo-url>/index.json.bundle`.
The index must use the current client schema:

```json
{
  "schema": 1,
  "index_version": 1,
  "generated_at": "2026-05-09T00:00:00Z",
  "expires_at": "2026-05-16T00:00:00Z",
  "packages": {
    "yurt-greet": {
      "sha256": "<sha256 of packages/yurt-greet.json>",
      "size": 1234,
      "url": "packages/yurt-greet.json"
    }
  }
}
```

Each package file must use the current client schema:

```json
{
  "name": "yurt-greet",
  "versions": [
    {
      "version": "0.1.0",
      "build": "yurt_0",
      "url": "artifacts/yurt-greet/0.1.0/yurt-greet-0.1.0-yurt_0.yurtpkg",
      "sha256": "<sha256 of artifact>",
      "size": 1234,
      "signing": {
        "subject": "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main",
        "issuer": "https://token.actions.githubusercontent.com"
      },
      "depends": [],
      "yanked": false
    }
  ]
}
```

The artifact URL may be relative. `pkg update` resolves it against the repo URL
when it later implements artifact fetch/install.

## CI Helper Shape

Add a `yurt-repo-ci publish-local` command for the first implementation. It
does not sign artifacts or talk to GitHub Releases. It takes:

- a package artifact path
- package identity fields
- repo root
- optional `--generated-at` for deterministic tests

It writes or updates:

- `artifacts/<name>/<version>/<artifact-name>`
- `packages/<name>.json`
- `index.json`
- `index.json.bundle`

`index.json.bundle` may be a placeholder for local testing because the current
`pkg --features test-fixtures update` path uses `StaticVerifier`; real Sigstore
signing remains a follow-up CI step.

The helper must validate its own output by deserializing with
`yurt-pkg-repo::metadata::{Index, PackageFile}` and running the same validation
the client uses.

## Yurt Greet Port

`yurt-ports/ports/yurt-greet` should be a recipe/build wrapper around the
upstream `YurtOS/yurt-greet` source, not a copied source tree. For local
developer acceptance, the build script may support `YURT_GREET_SOURCE` pointing
at `../yurt-greet`. CI can use the Git URL/ref declared in `port.toml`.

The port build flow is:

1. fetch or use the source checkout
2. run `cargo-yurt build --release`
3. stage the package root
4. run `yurt-pack build`
5. hand the generated `.yurtpkg` to `yurt-repo-ci publish-local`

## Acceptance

From the sibling checkouts:

```bash
YURT_GREET_SOURCE=../yurt-greet \
YURT_KERNEL_ROOT=../yurtos-kernel \
YURT_PKG_ROOT=../yurt-pkg \
  ../yurt-ports/ports/yurt-greet/scripts/package.sh
```

Then publish into `yurt-packages`:

```bash
cargo run -p yurt-repo-ci -- publish-local \
  --repo-root ../yurt-packages \
  --artifact ../yurt-ports/ports/yurt-greet/build/dist/yurt-greet-0.1.0-yurt_0.yurtpkg \
  --manifest ../yurt-ports/ports/yurt-greet/yurt-pack.toml \
  --generated-at 2026-05-09T00:00:00Z
```

Then verify the client path:

```bash
cargo run -p pkg --features test-fixtures -- \
  --etc-root /tmp/yurt-pkg-etc \
  --cache-root /tmp/yurt-pkg-cache \
  update
cargo run -p pkg --features test-fixtures -- \
  --etc-root /tmp/yurt-pkg-etc \
  --cache-root /tmp/yurt-pkg-cache \
  search yurt
cargo run -p pkg --features test-fixtures -- \
  --etc-root /tmp/yurt-pkg-etc \
  --cache-root /tmp/yurt-pkg-cache \
  info yurt-greet
```

Expected result: `pkg update` accepts the real `yurt-packages/index.json`,
`pkg search yurt` lists `yurt-greet`, and `pkg info yurt-greet` shows version
`0.1.0-yurt_0`.
