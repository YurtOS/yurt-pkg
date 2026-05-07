# Building Yurt Packages

Building a package has two stages:

1. Build Yurt-compatible `wasm32-wasip1` binaries into a staged root.
2. Run `yurt-pack` over that staged root to create a `.yurtpkg` archive.

The compiler toolchain lives in the sibling kernel repository:

```text
../yurtos-kernel/abi/toolchain/yurt-toolchain
```

It provides the Yurt ABI wrappers:

```text
yurt-cc       C/C++ compiler driver wrapper
yurt-ar       archive tool wrapper
yurt-ranlib   ranlib wrapper
cargo-yurt    Cargo wrapper for Rust wasm32-wasip1 builds
maturin-yurt  maturin wrapper for Python extension builds
yurt-check    ABI/conformance checker
yurt-conf     toolchain configuration helper
```

This repository provides the packaging layer:

```text
yurt-pack     staged-root -> .yurtpkg archive
```

## C And C++

Use `yurt-cc` as `CC` and the companion archive tools as `AR` and `RANLIB`.
`yurt-cc` wraps wasi-sdk clang with the Yurt ABI include paths, target, link
arguments, and post-link processing needed by the kernel.

Typical flow:

```bash
export YURT_KERNEL_ROOT=/path/to/yurtos-kernel
export YURT_PKG_ROOT=/path/to/yurt-pkg

export CC="$YURT_KERNEL_ROOT/target/release/yurt-cc"
export AR="$YURT_KERNEL_ROOT/target/release/yurt-ar"
export RANLIB="$YURT_KERNEL_ROOT/target/release/yurt-ranlib"

make

mkdir -p stage/bin
cp path/to/program.wasm stage/bin/program

cargo run -p yurt-pack -- build stage \
  --manifest yurt-pack.toml \
  --out dist
```

For Autotools or Makefile-based ports, pass the same wrappers through the
port's normal cross-compilation variables. The kernel repository has working
examples under:

```text
../yurtos-kernel/test-fixtures/c-ports/
```

## Rust

Use `cargo-yurt` for Rust projects. The wrapper drives real Cargo with the
`wasm32-wasip1` target and Yurt linker/runtime configuration.

Typical flow:

```bash
export YURT_KERNEL_ROOT=/path/to/yurtos-kernel
export YURT_PKG_ROOT=/path/to/yurt-pkg

"$YURT_KERNEL_ROOT/target/release/cargo-yurt" build --release

mkdir -p stage/bin
cp target/wasm32-wasip1/release/my-tool.wasm stage/bin/my-tool

cargo run -p yurt-pack -- build stage \
  --manifest yurt-pack.toml \
  --out dist
```

## Staged Root Rules

The staged root should look like the final sandbox filesystem fragment:

```text
stage/
  bin/my-tool
  usr/share/my-tool/data.txt
```

`yurt-pack` walks that tree, records file metadata in `info/files.json`, and
writes the package archive. Authors can also declare hardlinks explicitly in
`yurt-pack.toml` when a staged filesystem cannot preserve inode identity.

## Package Manifest

`yurt-pack.toml` supplies package identity and runtime requirements:

```toml
name        = "my-tool"
version     = "0.1.0"
build       = "yurt_0"
platform    = "wasm32-wasip1-yurt"
summary     = "Example Yurt tool"
license     = "Apache-2.0"
default_uid = 0
default_gid = 0

[depends]
libc = "^0.1"

[yurt]
commands = ["my-tool"]

[yurt.requires]
network   = false
processes = false
threads   = false
```

Build the package:

```bash
yurt-pack build stage --manifest yurt-pack.toml --out dist
```

The output is:

```text
dist/my-tool-0.1.0-yurt_0.yurtpkg
```

Signing and repository publication are handled by repository CI, not by local
developer builds in this slice.
