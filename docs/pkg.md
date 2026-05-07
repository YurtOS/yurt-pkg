# `pkg` Command

`pkg` is the in-sandbox package client for Yurt. This PR adds the command
surface and the shared metadata/trust crates it will use, but most executable
behavior is intentionally deferred to follow-up specs.

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

## Deferred Behavior

Current stubs fail with explicit messages for behavior not implemented in this
slice:

- `pkg update`, `pkg search`, and `pkg info` are owned by
  `docs/superpowers/specs/2026-05-07-yurt-pkg-update-flow-design.md`.
- `pkg install`, `pkg upgrade`, `pkg remove`, and `pkg list` are owned by
  `docs/superpowers/specs/2026-05-07-yurt-pkg-resolver-installer-design.md`.

The update-flow spec owns HTTP fetching, 304 freshness revalidation,
`meta.json` persistence, rollback checks against persisted
`last_index_version`, and `db.sqlite` cache writes.

The resolver/installer spec owns dependency solving, yanked-version behavior,
install transactions, installed-state mutation, and atomic filesystem updates.

## Repository Priority

When multiple trusted repositories publish the same package, lower numeric
priority wins. Ties are broken by repository id to keep selection
deterministic.
