# Yurt Package Sigstore Fallback Verifier Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the Sigstore Bundle verification path for the in-sandbox client after resolving the WASI build prerequisites.

**Architecture:** Keep `yurt-pkg-repo::verify::BundleVerifier` as the stable integration boundary. First make `yurt-pkg-repo` buildable for `wasm32-wasip1` without pulling archive writer dependencies that require native zstd compilation, then choose either an upstream `sigstore`-backed verifier or a local fallback verifier against pinned Fulcio/Rekor roots.

**Tech Stack:** Rust 2021, Sigstore Bundle JSON/protobuf parsing, Fulcio certificate-chain validation, Rekor SET/inclusion proof verification, ECDSA-P256 signature verification.

---

## Smoke-Test Result

On 2026-05-07, `cargo check -p yurt-pkg-repo --target wasm32-wasip1` with `sigstore = { version = "0.11", default-features = false }` failed before Sigstore-specific APIs were evaluated.

The immediate blocker was the existing `yurt-pkg-repo -> yurt-pkg-format -> zstd -> zstd-sys` dependency path. `zstd-sys` attempted to invoke clang for `--target=wasm32-wasip1` and failed with `unknown target triple 'wasm32-unknown-wasip1'` / `No available targets are compatible with triple "wasm32-unknown-wasip1"`.

## Tasks

- [ ] Split reusable manifest metadata helpers out of `yurt-pkg-format` or gate archive read/write dependencies so `yurt-pkg-repo` can compile for `wasm32-wasip1` without native `zstd-sys`.
- [ ] Re-run the upstream Sigstore compile smoke test against the reduced `yurt-pkg-repo` dependency graph.
- [ ] If upstream Sigstore compiles, implement `SigstoreVerifier` behind `BundleVerifier`.
- [ ] If upstream Sigstore still does not compile, implement the fallback verifier described in `docs/superpowers/specs/2026-05-07-yurt-pkg-distribution-design.md`: bundle parsing, Fulcio chain validation, subject/issuer extraction, signature verification, and Rekor proof verification.
- [ ] Add fixture bundles signed by a test identity and negative cases for wrong subject, wrong issuer, wrong payload hash, expired cert window, and invalid Rekor proof.
