# Yurt Packages CI Repository Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a first end-to-end local repository loop where `yurt-ports` packages `yurt-greet`, `yurt-packages` receives generated metadata, and `pkg update/search/info` consumes it.

**Architecture:** Add metadata generation to the existing `yurt-repo-ci` CLI, using `yurt-pack.toml` and a built `.yurtpkg` artifact as input. Keep `yurt-ports` source-light by making `ports/yurt-greet` build from an external source checkout, with a local `YURT_GREET_SOURCE` override for acceptance testing.

**Tech Stack:** Rust workspace in `yurt-pkg`, shell scripts in `yurt-ports`, JSON/TOML metadata, local `file://` repository access through existing `pkg --features test-fixtures`.

---

### Task 1: Add Repository Publish Metadata Generation

**Files:**
- Modify: `crates/yurt-repo-ci/Cargo.toml`
- Modify: `crates/yurt-repo-ci/src/main.rs`
- Test: `crates/yurt-repo-ci/tests/publish_local.rs`

- [ ] **Step 1: Add dependencies for hashing and time parsing**

Add these dependencies to `crates/yurt-repo-ci/Cargo.toml`:

```toml
hex = { workspace = true }
sha2 = { workspace = true }
time = { workspace = true }
```

- [ ] **Step 2: Write failing CLI test for `publish-local`**

Create `crates/yurt-repo-ci/tests/publish_local.rs` with a test that:

1. creates a temp repo root
2. writes a fake `.yurtpkg` artifact
3. writes a minimal `yurt-pack.toml`
4. runs `yurt-repo-ci publish-local`
5. asserts `index.json`, `index.json.bundle`, `packages/yurt-greet.json`, and artifact copy exist
6. deserializes `index.json` and `packages/yurt-greet.json`

- [ ] **Step 3: Run the failing test**

Run:

```bash
cargo test -p yurt-repo-ci --test publish_local
```

Expected: fail because `publish-local` is not a known subcommand.

- [ ] **Step 4: Implement `publish-local`**

In `crates/yurt-repo-ci/src/main.rs`:

- add a `PublishLocal` subcommand with `--repo-root`, `--artifact`, `--manifest`, and optional `--generated-at`
- parse the package fields from `yurt-pack.toml`
- compute SHA-256 and size for the artifact
- copy the artifact to `artifacts/<name>/<version>/<artifact-name>`
- append or replace the matching version/build entry in `packages/<name>.json`
- regenerate `index.json`
- write placeholder `index.json.bundle`
- validate generated `Index` and `PackageFile`

- [ ] **Step 5: Run publish-local tests**

Run:

```bash
cargo test -p yurt-repo-ci --test publish_local
```

Expected: pass.

### Task 2: Make `yurt-greet` Port Source-Light

**Files:**
- Modify: `../yurt-ports/ports/yurt-greet/port.toml`
- Modify: `../yurt-ports/ports/yurt-greet/scripts/build.sh`
- Modify: `../yurt-ports/ports/yurt-greet/scripts/package.sh`
- Remove copied source files from `../yurt-ports/ports/yurt-greet/src/`, `Cargo.toml`, and `Cargo.lock` if the script no longer uses them

- [ ] **Step 1: Update `port.toml` source metadata**

Set source metadata to the upstream Git repo and keep local override documented:

```toml
[source]
type = "git"
url = "https://github.com/YurtOS/yurt-greet.git"
rev = "main"
local_override_env = "YURT_GREET_SOURCE"
```

- [ ] **Step 2: Update build script**

Change `scripts/build.sh` so it:

- resolves source from `YURT_GREET_SOURCE`, or clones/fetches into `build/source/yurt-greet`
- runs `cargo-yurt build --release` in that source
- stages `crates/yurt-greet` output and source `stage/` files into `build/stage`

- [ ] **Step 3: Update package script**

Keep `scripts/package.sh` calling `scripts/build.sh`, then run `yurt-pack build build/stage --manifest yurt-pack.toml --out build/dist`.

- [ ] **Step 4: Run package script with local source override**

Run:

```bash
YURT_GREET_SOURCE=/Users/sunny/work/yurtos/yurt-greet \
YURT_KERNEL_ROOT=/Users/sunny/work/yurtos/yurtos-kernel \
YURT_PKG_ROOT=/Users/sunny/work/yurtos/yurt-pkg \
  /Users/sunny/work/yurtos/yurt-ports/ports/yurt-greet/scripts/package.sh
```

Expected: `build/dist/yurt-greet-0.1.0-yurt_0.yurtpkg` exists.

### Task 3: Seed `yurt-packages` Through the CI Helper

**Files:**
- Modify: `../yurt-packages/README.md`
- Generate: `../yurt-packages/index.json`
- Generate: `../yurt-packages/index.json.bundle`
- Generate: `../yurt-packages/packages/yurt-greet.json`
- Generate: `../yurt-packages/artifacts/yurt-greet/0.1.0/yurt-greet-0.1.0-yurt_0.yurtpkg`

- [ ] **Step 1: Run publish-local**

Run:

```bash
cargo run -p yurt-repo-ci -- publish-local \
  --repo-root /Users/sunny/work/yurtos/yurt-packages \
  --artifact /Users/sunny/work/yurtos/yurt-ports/ports/yurt-greet/build/dist/yurt-greet-0.1.0-yurt_0.yurtpkg \
  --manifest /Users/sunny/work/yurtos/yurt-ports/ports/yurt-greet/yurt-pack.toml \
  --generated-at 2026-05-09T00:00:00Z
```

Expected: generated repository files exist and validate.

- [ ] **Step 2: Update README**

Document that generated metadata is CI-owned and `publish-local` is the local stand-in for the first loop.

### Task 4: Exercise `pkg` Against `yurt-packages`

**Files:**
- Temporary only under `/private/tmp/yurt-packages-acceptance`

- [ ] **Step 1: Write trusted repo config**

Create `/private/tmp/yurt-packages-acceptance/etc/yurt-pkg/trusted-repos.toml` pointing at `file:///Users/sunny/work/yurtos/yurt-packages/`, with the v1 signing subject and issuer.

- [ ] **Step 2: Run update**

Run:

```bash
cargo run -p pkg --features test-fixtures -- \
  --etc-root /private/tmp/yurt-packages-acceptance/etc \
  --cache-root /private/tmp/yurt-packages-acceptance/cache \
  update
```

Expected: prints `updated official`.

- [ ] **Step 3: Run search**

Run:

```bash
cargo run -p pkg --features test-fixtures -- \
  --etc-root /private/tmp/yurt-packages-acceptance/etc \
  --cache-root /private/tmp/yurt-packages-acceptance/cache \
  search yurt
```

Expected: output contains `yurt-greet 0.1.0-yurt_0 official`.

- [ ] **Step 4: Run info**

Run:

```bash
cargo run -p pkg --features test-fixtures -- \
  --etc-root /private/tmp/yurt-packages-acceptance/etc \
  --cache-root /private/tmp/yurt-packages-acceptance/cache \
  info yurt-greet
```

Expected: output contains `yurt-greet`, `repo: official`, and `0.1.0-yurt_0`.

### Task 5: Final Verification and Commits

**Files:**
- yurt-pkg changes from Tasks 1 and spec/plan
- yurt-ports changes from Task 2
- yurt-packages generated repository files from Task 3

- [ ] **Step 1: Run yurt-pkg tests**

Run:

```bash
cargo test -p yurt-repo-ci --tests
cargo test --tests
```

Expected: pass.

- [ ] **Step 2: Check worktrees**

Run:

```bash
git -C /Users/sunny/work/yurtos/yurt-pkg status --short
git -C /Users/sunny/work/yurtos/yurt-ports status --short
git -C /Users/sunny/work/yurtos/yurt-packages status --short
```

- [ ] **Step 3: Commit each repo separately**

Use separate commits:

```bash
git -C /Users/sunny/work/yurtos/yurt-pkg add ...
git -C /Users/sunny/work/yurtos/yurt-pkg commit -m "feat: generate local package repository metadata"
git -C /Users/sunny/work/yurtos/yurt-ports add ...
git -C /Users/sunny/work/yurtos/yurt-ports commit -m "feat: port yurt-greet from source checkout"
git -C /Users/sunny/work/yurtos/yurt-packages add ...
git -C /Users/sunny/work/yurtos/yurt-packages commit -m "feat: publish yurt-greet package metadata"
```
