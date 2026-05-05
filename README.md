# yurt-pkg

Package format and packaging tools for [YurtOS](https://github.com/YurtOS/yurtos-kernel).

This repository is **outside** the kernel boundary. The kernel only knows how
to apply a prevalidated tar payload to its VFS for boot fixtures and tests.
Everything else — manifests, dependency constraints, registry indexes, the
`pkg install` command — lives here.

## Format

A package is a zstd-compressed tar archive named:

```
<name>-<version>-<build>.yurtpkg.tar.zst
```

with this layout:

```
info/index.json       # required: name, version, build, platform, depends, …
info/files.json       # required: per-entry sha256 / size / mode / uid / gid
info/yurt.json        # optional: runtime requirements
bin/, usr/, etc/, …   # archive entries map 1:1 to VFS paths
```

Tar entries (regular files, directories, symlinks, hardlinks) are the source
of truth for installed filesystem state. Modes, uids, and gids are preserved.
Yurt's canonical ownership values are `0:0` for system tools and `1000:1000`
for user-owned data.

The full normative design lives in
[`yurtos-kernel/docs/superpowers/specs/2026-05-05-yurt-package-format-design.md`](https://github.com/YurtOS/yurtos-kernel/blob/main/docs/superpowers/specs/2026-05-05-yurt-package-format-design.md).

## Crates

- `yurt-pkg-format` — read, validate, and write `.yurtpkg.tar.zst` archives.
  Pure library; no fs side effects beyond the I/O the caller hands it.
- `yurt-pack` — host CLI that builds a package archive from a source tree
  and a manifest file.

The in-sandbox `pkg` binary (`install` / `remove` / `list` / `info`) and the
registry index server are deliberately out of scope for the first cut and
will land as follow-ups.

## Usage

Build a package from a staged tree:

```bash
yurt-pack build path/to/staged-root \
  --manifest path/to/yurt-pack.toml \
  --out dist/
```

`yurt-pack.toml` provides the metadata that `info/index.json` and (optionally)
`info/yurt.json` need:

```toml
name        = "busybox"
version     = "1.36.1"
build       = "yurt_0"
platform    = "wasm32-wasip1-yurt"
summary     = "BusyBox userland tools for Yurt"
license     = "GPL-2.0-only"
depends     = []

[yurt]
min_yurt_version = "0.1.0"
commands         = ["busybox", "ash", "sh", "cat"]

[yurt.requires]
network   = false
processes = true
threads   = false
```

## License

Apache-2.0.
