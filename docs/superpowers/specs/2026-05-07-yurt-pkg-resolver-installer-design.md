# Yurt Package Resolver and Installer Design

**Status:** Stub
**Date:** 2026-05-07

## Scope

This follow-up spec owns resolver algorithm internals, transaction planning,
file ownership collision handling, install root atomicity strategy, installed
database schema, removal semantics, and install hooks.

The distribution design fixes the user-visible contract:

- install and upgrade include transitive dependencies;
- already-installed packages contribute constraints to the solve;
- version collisions abort before filesystem mutation;
- yanked versions are skipped unless explicitly allowed;
- failed install/upgrade transactions leave no partial state visible.

## Open Decisions

- Solver strategy and backtracking order.
- How package repo priority affects dependency selection when multiple trusted
  repos provide the same package name.
- Whether installed packages may move between repos during upgrade.
- Atomic install strategy for the Yurt filesystem.
- `installed.sqlite` schema and migration policy.
