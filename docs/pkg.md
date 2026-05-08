# `pkg` Command

`pkg` is the in-sandbox package client for Yurt. It can refresh trusted
repository metadata into a local cache and answer offline search/info queries.
Package installation and installed-state mutation are still deferred to the
resolver/installer spec.

## Command Surface

```text
pkg update
pkg search <query>
pkg info <name>
pkg install <name>[@<version>]
pkg upgrade [<name>...]
pkg remove <name>
pkg list [--yanked]
pkg add-repo <url> --signing-subject <s> --signing-issuer <i> [--id <name>] [--priority <n>]
```

`pkg add-repo` requires both `--signing-subject` and `--signing-issuer`.
There are no defaults and no interactive prompt because repository trust is a
privileged operation.

## Trust Inputs

The client trust model is rooted in files written at image build time:

```text
/etc/yurt-pkg/trusted-repos.toml
/etc/yurt-pkg/sigstore-trust-root/
```

`trusted-repos.toml` pins each trusted repository by URL, priority, signing
subject, and signing issuer:

```toml
[[repo]]
id              = "official"
url             = "https://github.com/YurtOS/yurt-packages"
priority        = 0
signing_subject = "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main"
signing_issuer  = "https://token.actions.githubusercontent.com"
```

Both subject and issuer must match signed repository metadata and package
artifacts. Yurt does not pin per-package public keys.

## Repository Cache

`pkg update` reads `/etc/yurt-pkg/trusted-repos.toml`, verifies each trusted
repository's signed `index.json`, fetches package metadata JSON files, and
commits an immutable cache snapshot under:

```text
/var/cache/yurt-pkg/repos/<repo-id>/
```

`pkg search` and `pkg info` read only this local cache. They do not touch the
network. If a cache is stale or the previous update failed, they print warnings
while still rendering usable cached metadata. If no cache exists, they exit
nonzero and suggest running `pkg update`.

`pkg info <name> --repo <id>` limits output to one trusted repository.

## Deferred Behavior

Current stubs fail with explicit messages for behavior not implemented in this
slice:

- `pkg install`, `pkg upgrade`, `pkg remove`, and `pkg list` are owned by
  `docs/superpowers/specs/2026-05-07-yurt-pkg-resolver-installer-design.md`.

The resolver/installer spec owns dependency solving, yanked-version behavior,
install transactions, installed-state mutation, and atomic filesystem updates.

## Repository Priority

When multiple trusted repositories publish the same package, lower numeric
priority wins. Ties are broken by repository id to keep selection
deterministic.
