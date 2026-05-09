# CI Package Publish And Pkg Port Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish packages through `yurt-packages` CI, reject duplicate releases, and add `pkg` itself as the second source-light port.

**Architecture:** Keep release authority in `yurt-packages`. `yurt-repo-ci publish-local` remains the metadata generator, but gains an explicit duplicate rejection mode for CI. Ports stay source-light in `yurt-ports`, with sources checked out beside the package repository during CI.

**Tech Stack:** Rust CLI (`yurt-repo-ci`), GitHub Actions, shell port scripts, `cargo-yurt`, `yurt-pack`, local `pkg --features test-fixtures` verification.

---

### Task 1: CI Duplicate Guard

**Files:**
- Modify: `crates/yurt-repo-ci/src/main.rs`
- Modify: `crates/yurt-repo-ci/tests/publish_local.rs`

- [ ] Add `--reject-existing` to `publish-local`.
- [ ] If the package file already contains the same `version` and `build`, fail before copying the artifact or mutating metadata.
- [ ] Keep the default replacement behavior for local regeneration.
- [ ] Add one test proving a duplicate publish fails with `--reject-existing`.
- [ ] Run `cargo test -p yurt-repo-ci --test publish_local`.

### Task 2: Central Publish Workflow

**Files:**
- Create: `/Users/sunny/work/yurtos/yurt-packages/.github/workflows/publish-package.yml`
- Modify: `/Users/sunny/work/yurtos/yurt-packages/README.md`

- [ ] Add a manual `workflow_dispatch` workflow with `package` input.
- [ ] Check out `YurtOS/yurt-packages`, `YurtOS/yurt-ports`, `YurtOS/yurt-pkg`, `YurtOS/yurtos-kernel`, and the selected source repository.
- [ ] Build `cargo-yurt` and the Yurt Rust std in the kernel checkout.
- [ ] Run the selected port `scripts/package.sh`.
- [ ] Run `yurt-repo-ci publish-local --reject-existing`.
- [ ] Commit and push generated repository changes when present.
- [ ] Document that CI owns publishing and duplicate releases fail.

### Task 3: `pkg` Source-Light Port

**Files:**
- Create: `/Users/sunny/work/yurtos/yurt-ports/ports/pkg/port.toml`
- Create: `/Users/sunny/work/yurtos/yurt-ports/ports/pkg/yurt-pack.toml`
- Create: `/Users/sunny/work/yurtos/yurt-ports/ports/pkg/scripts/build.sh`
- Create: `/Users/sunny/work/yurtos/yurt-ports/ports/pkg/scripts/package.sh`
- Create: `/Users/sunny/work/yurtos/yurt-ports/ports/pkg/README.md`

- [ ] Build `pkg` from `YurtOS/yurt-pkg` using `cargo-yurt`.
- [ ] Stage the wasm binary at `build/stage/bin/pkg`.
- [ ] Package with `yurt-pack`.
- [ ] Keep the port source-light; no cloned source files live under `ports/pkg`.

### Task 4: Local Acceptance

**Files:**
- No committed files expected outside tasks 1-3.

- [ ] Run `pre-commit run --all-files` in `yurt-pkg`.
- [ ] Run `cargo test --tests` in `yurt-pkg`.
- [ ] Build/package `yurt-greet` from `yurt-ports`.
- [ ] Confirm `publish-local --reject-existing` fails for already-published `yurt-greet`.
- [ ] Build/package `pkg` from `yurt-ports`.
- [ ] Publish `pkg` locally into `yurt-packages`.
- [ ] Verify `pkg update`, `pkg search`, and `pkg info pkg` using a temp trust config with no committed local URL.

### Task 5: Push And CI

**Files:**
- Commits in `yurt-pkg`, `yurt-ports`, and `yurt-packages`.

- [ ] Commit each repository separately.
- [ ] Push each repository to `main`.
- [ ] Run `gh workflow run publish-package.yml -f package=yurt-greet` and confirm it fails for duplicate version/build.
- [ ] Run `gh workflow run publish-package.yml -f package=pkg` and confirm it succeeds.
- [ ] Confirm `yurt-pkg` GitHub Actions are green.
