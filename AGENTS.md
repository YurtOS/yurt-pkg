# Agent Instructions — yurt-pkg

Canonical instructions for AI coding agents (Claude Code, Codex, etc.) working in this repo. `CLAUDE.md` points here.

## Project shape

- **Language:** Rust (workspace at repo root, `Cargo.toml`).
- **Crates:** `crates/yurt-pkg-format` (on-disk format types), `crates/yurt-pack` (CLI / packing logic).
- **Toolchain pin:** Rust `1.95.0` (see `.github/workflows/rust.yml`).
- **Scope:** package format and tooling for YurtOS. Distribution/trust design lives under `docs/`.

## The bar: CI green = done

A change is not done until every required CI job is green. The gates are:

- `.github/workflows/rust.yml` — `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --tests`.

If a job is red, the work is not done. Do not claim completion based on a local pass alone — always verify the PR's checks. "Flaky, will retry" is not a resolution; investigate the failure.

## Local gates: run fast tests before you push

Hooks are managed by [`pre-commit`](https://pre-commit.com) and configured in `.pre-commit-config.yaml`. Install once:

```bash
scripts/install-hooks.sh
```

That registers both stages:

- **pre-commit** — `cargo fmt --check`, `cargo clippy -D warnings` (changed crates via `scripts/lint-clippy-changed.sh`), plus generic hygiene (trailing whitespace, EOF, merge markers, large-file guard).
- **pre-push** — fast-tier tests: `cargo test --tests`.

Never bypass with `--no-verify`. If a hook fails, fix the underlying issue and create a new commit; do not amend over a hook failure.

## Development procedure (non-trivial work)

For any feature, refactor, or bugfix beyond a one-line tweak, follow the superpowers loop in order. Each step has a skill — invoke it via the `Skill` tool.

1. **Brainstorm** — `superpowers:brainstorming`. Clarify intent, requirements, alternatives, tradeoffs *before* committing to a design.
2. **Plan** — `superpowers:writing-plans`. Produce a written plan under `docs/superpowers/plans/YYYY-MM-DD-<slug>.md`. Specs (when separate from plans) go under `docs/superpowers/specs/`.
3. **Implement with TDD** — `superpowers:test-driven-development`. Red → green → refactor. Tests live next to code in `#[cfg(test)] mod tests`.
4. **Verify before claiming done** — `superpowers:verification-before-completion`. Run the full local gate set (`pre-commit run --all-files` and the pre-push tests) and confirm output. Evidence before assertions.
5. **Request review** — `superpowers:requesting-code-review` for substantive changes; `code-review:code-review` for PR-level review.
6. **Receive review** — `superpowers:receiving-code-review`. Address feedback with technical rigor; verify before agreeing.

For multi-agent independent work, use `superpowers:dispatching-parallel-agents` and `subagent-driven-development`.
For bugs, start with `superpowers:systematic-debugging` before proposing fixes.

## Code standards

### Rust

Follow `coding-guidelines` (skill) for style, naming, comments, and clippy expectations. For language-feature questions, invoke the relevant module skill from the `actionbook/rust-skills` plugin (pre-registered — see [`.claude/settings.json`](.claude/settings.json)) rather than guessing:

| Topic | Skill |
| --- | --- |
| Ownership / borrows / lifetimes | `m01-ownership` |
| Smart pointers, RAII, `Drop` | `m02-resource`, `m12-lifecycle` |
| Mutability and interior mutability | `m03-mutability` |
| Generics, traits, zero-cost abstractions | `m04-zero-cost` |
| Concurrency, async, channels, locks | `m07-concurrency` |
| Performance, allocations, benchmarking | `m10-performance` |
| Cargo, features, workspaces, ecosystem crates | `m11-ecosystem` |
| Error handling | `error-handling-patterns` |
| Memory safety patterns | `memory-safety-patterns` |

Repo-specific Rust rules:

- **Prefer Rust over C.** When a piece of functionality could be written in either, choose Rust. Reach for C only when interop forces it (existing C library with no Rust wrapper, ABI requirement). Document the reason in a comment when you do.
- **Minimize `unsafe`.** Each `unsafe` block weakens the soundness story for the whole crate, so keep them rare, small, and isolated. Before writing one, check whether a safe abstraction (existing crate, `bytemuck`, `zerocopy`, slice methods, `MaybeUninit` patterns) covers the case. When `unsafe` is unavoidable: keep the block as narrow as possible, write a `// SAFETY:` comment that names the invariants the caller relies on, and confine it behind a safe wrapper so callers don't have to think about it. Do not use `unsafe` for performance without a benchmark that demonstrates the win.
- Workspace members are listed explicitly in `Cargo.toml`.
- Clippy is `-D warnings` in CI. Don't `#[allow(...)]` to silence a warning unless there's a real reason; document it in a one-line comment when you do.
- The pre-commit clippy hook only lints crates touched in the diff (`scripts/lint-clippy-changed.sh`). Run `cargo clippy --all-targets` locally before pushing if your change crosses crate boundaries.

## Tests

- **Fast tier** (runs on pre-push and in CI): `cargo test --tests`. Keep this under a few minutes.
- New tests should be deterministic. If a test depends on locale, time, hostname, or network, isolate it.

## Conventions

- Plans: `docs/superpowers/plans/YYYY-MM-DD-<slug>.md`. Specs: `docs/superpowers/specs/YYYY-MM-DD-<slug>-design.md`.
- Commits: short imperative subject (`feat:`, `fix:`, `docs:`, `chore:`). Squash WIP before pushing for review.
- Don't add files outside the documented locations without a reason. Don't create markdown files speculatively.

## When in doubt

Read the skill before acting. If a skill applies, invoking it is mandatory, not optional — see `superpowers:using-superpowers`.
