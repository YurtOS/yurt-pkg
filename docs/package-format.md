# Yurt Package Format

A Yurt package is a zstd-compressed tar archive with a canonical published
basename:

```text
<name>-<version>-<build>.yurtpkg
```

The file extension is intentionally `.yurtpkg`, even though the payload is
still a zstd-compressed tar stream. Release artifacts and repository metadata
must use the `.yurtpkg` basename, with Sigstore bundles published as
`<name>-<version>-<build>.yurtpkg.bundle`.

## Archive Layout

```text
info/index.json       # required package identity and dependency metadata
info/files.json       # required installed file manifest
info/yurt.json        # optional runtime requirements
bin/, usr/, etc/      # payload entries installed under the sandbox root
```

`info/index.json` is the package identity manifest. The current schema version
is `2`, which represents dependencies as structured objects:

```json
{
  "schema_version": 2,
  "name": "busybox",
  "version": "1.36.1",
  "build": "yurt_0",
  "platform": "wasm32-wasip1-yurt",
  "summary": "BusyBox userland tools for Yurt",
  "license": "GPL-2.0-only",
  "depends": [
    { "name": "libc", "req": "^0.1" }
  ]
}
```

`info/files.json` records every installed path, type, mode, uid, gid, and for
regular files the SHA-256 digest and size. The archive reader validates that
the tar payload matches this manifest.

`info/yurt.json` is optional and records runtime requirements that are not file
system facts, such as command names and whether the package requires network,
process, or thread support.

## Naming And Validation

Package names must match:

```text
^[a-z0-9][a-z0-9._-]*$
```

Archive paths must be relative, normalized, non-empty, and must not escape the
sandbox root. Duplicate paths are rejected after normalization. Symlink and
hardlink targets are validated separately.

Modes are canonical four-character octal strings such as `0755`, `0644`, or
`0000`. Yurt currently models two canonical ownership pairs:

```text
0:0       system/root-owned files
1000:1000 user-owned files
```

`yurt-pack` warns when staged files use non-canonical ownership and refuses to
silently invent default ownership. Authors must provide `default_uid` and
`default_gid` in `yurt-pack.toml`.

## Builder Manifest

`yurt-pack.toml` is the host-side input to `yurt-pack`. It is not copied into
the package. `yurt-pack` translates it into `info/index.json`,
`info/files.json`, and optional `info/yurt.json`.

```toml
name        = "busybox"
version     = "1.36.1"
build       = "yurt_0"
platform    = "wasm32-wasip1-yurt"
summary     = "BusyBox userland tools for Yurt"
license     = "GPL-2.0-only"
default_uid = 0
default_gid = 0

[depends]
libc = "^0.1"

[yurt]
min_yurt_version = "0.1.0"
commands         = ["busybox", "ash", "sh"]

[yurt.requires]
network   = false
processes = true
threads   = false
```

For transition only, `yurt-pack` still accepts legacy `depends = []` and
legacy string-array entries. New manifests should use the `[depends]` table.

## Normative Specs

The package archive format was originally specified in the kernel repository:

- `../yurtos-kernel/docs/superpowers/specs/2026-05-05-yurt-package-format-design.md`

Distribution metadata, signing, and repository layout are specified here:

- `docs/superpowers/specs/2026-05-07-yurt-pkg-distribution-design.md`
