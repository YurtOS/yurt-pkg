#!/usr/bin/env bash
set -euo pipefail

# Run cargo clippy scoped to crates that have staged Rust changes.
# Falls back to a full workspace lint if no Rust files are staged
# (e.g. when invoked via `pre-commit run --all-files`).

CHANGED="$(git diff --cached --name-only --diff-filter=ACMRT -- '*.rs' || true)"

if [[ -z "$CHANGED" ]]; then
  # Default-members only — wasm-only canary crates need a different target.
  exec cargo clippy --all-targets -- -D warnings
fi

# Map each changed file to the nearest *package* Cargo.toml directory.
# Virtual manifests (workspace-only Cargo.toml with no `name = ...`) are
# skipped — keep walking up.
CRATE_PKGS="$(mktemp "${TMPDIR:-/tmp}/yurt-pkg-clippy.XXXXXX")"
trap 'rm -f "$CRATE_PKGS"' EXIT
while IFS= read -r f; do
  dir="$(dirname "$f")"
  while [[ "$dir" != "." && "$dir" != "/" ]]; do
    if [[ -f "$dir/Cargo.toml" ]]; then
      pkg="$(awk -F\" '/^name *=/ {print $2; exit}' "$dir/Cargo.toml")"
      if [[ -n "$pkg" ]]; then
        printf '%s\n' "$pkg" >>"$CRATE_PKGS"
        break
      fi
    fi
    dir="$(dirname "$dir")"
  done
done <<<"$CHANGED"

if [[ ! -s "$CRATE_PKGS" ]]; then
  # Default-members only — wasm-only canary crates need a different target.
  exec cargo clippy --all-targets -- -D warnings
fi

sort -u "$CRATE_PKGS" | while IFS= read -r pkg; do
  echo "→ cargo clippy -p $pkg --all-targets -- -D warnings"
  cargo clippy -p "$pkg" --all-targets -- -D warnings
done
