# BusyBox Package Port Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish BusyBox as a Yurt package with upstream BusyBox versioning and applet symlinks.

**Architecture:** Move the build recipe source of truth into `YurtOS/yurt-ports` while keeping it source-light: scripts, config, and manifest only. `YurtOS/yurt-packages` CI builds that port, publishes `busybox` metadata and artifacts, then the kernel fixture can be removed once consumers use the package.

**Tech Stack:** Rust `yurt-pack`/`yurt-repo-ci`, shell port scripts, BusyBox 1.37.0, `yurt-cc`, GitHub Actions.

---

### Task 1: Add Source-Light BusyBox Port

**Files:**
- Create: `/Users/sunny/work/yurtos/yurt-ports/ports/busybox/README.md`
- Create: `/Users/sunny/work/yurtos/yurt-ports/ports/busybox/port.toml`
- Create: `/Users/sunny/work/yurtos/yurt-ports/ports/busybox/yurt-pack.toml`
- Create: `/Users/sunny/work/yurtos/yurt-ports/ports/busybox/busybox.config`
- Create: `/Users/sunny/work/yurtos/yurt-ports/ports/busybox/manifest.json`
- Create: `/Users/sunny/work/yurtos/yurt-ports/ports/busybox/scripts/build.sh`
- Create: `/Users/sunny/work/yurtos/yurt-ports/ports/busybox/scripts/package.sh`

- [ ] **Step 1: Copy the current kernel BusyBox config and manifest**

Use the existing Yurt-tested applet list and config from `/Users/sunny/work/yurtos/yurtos-kernel/test-fixtures/c-ports/busybox/`.

- [ ] **Step 2: Write the build script**

The script downloads BusyBox `1.37.0`, configures it with `busybox.config`, builds with `yurt-cc`, stages `build/stage/bin/busybox`, and creates one relative symlink per applet in `manifest.json`.

- [ ] **Step 3: Write package metadata**

Set `name = "busybox"`, `version = "1.37.0"`, `build = "yurt_0"`, `platform = "wasm32-wasip1-yurt"`, root ownership, and `commands` equal to the BusyBox applets plus `busybox`.

- [ ] **Step 4: Package locally**

Run:

```bash
YURT_KERNEL_ROOT=/Users/sunny/work/yurtos/yurtos-kernel \
YURT_ROOT=/Users/sunny/work/yurtos/yurtos-kernel \
YURT_PKG_ROOT=/Users/sunny/work/yurtos/yurt-pkg \
ports/busybox/scripts/package.sh
```

Expected: `ports/busybox/build/dist/busybox-1.37.0-yurt_0.yurtpkg`.

### Task 2: Publish Through CI

**Files:**
- Modify: `/Users/sunny/work/yurtos/yurt-packages/.github/workflows/publish-package.yml`

- [ ] **Step 1: Add `busybox` as a workflow package option**

Keep the generic workflow path; no BusyBox-specific checkout is needed because the port downloads upstream sources.

- [ ] **Step 2: Push `yurt-ports` and `yurt-packages`**

Commit the port and workflow update separately.

- [ ] **Step 3: Run the publish workflow**

Run:

```bash
gh workflow run publish-package.yml --repo YurtOS/yurt-packages -f package=busybox
```

Expected: CI commits `busybox` into `yurt-packages`.

### Task 3: Verify Repository Install

**Files:**
- No source files unless verification exposes a bug.

- [ ] **Step 1: Pull the generated package repository update**

Run `git pull --ff-only` in `/Users/sunny/work/yurtos/yurt-packages`.

- [ ] **Step 2: Inspect metadata**

Confirm `index.json`, `packages/busybox.json`, and `artifacts/busybox/1.37.0/busybox-1.37.0-yurt_0.yurtpkg` exist.

- [ ] **Step 3: Install and smoke test in a sandbox**

Use the existing local package repository flow to `pkg update`, install `busybox`, and run representative symlink commands such as `sh`, `echo`, and `true`.

### Task 4: Remove Kernel C-Port Fixture

**Files:**
- Modify/remove BusyBox references under `/Users/sunny/work/yurtos/yurtos-kernel/test-fixtures/c-ports/busybox`
- Modify kernel docs/tests only after package-based verification proves the replacement path.

- [ ] **Step 1: Identify all kernel references**

Run:

```bash
rg -n "test-fixtures/c-ports/busybox|busybox.manifest|busybox.wasm|c-ports/busybox" /Users/sunny/work/yurtos/yurtos-kernel
```

- [ ] **Step 2: Replace fixture ownership with package ownership**

Remove the duplicated C-port recipe and update docs/tests to reference the BusyBox package as source of truth.

- [ ] **Step 3: Verify kernel tests affected by BusyBox**

Run the smallest BusyBox-related kernel test set first, then broader checks if files changed outside docs.
