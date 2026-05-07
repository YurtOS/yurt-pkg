# Yurt Package Update Flow Design

**Status:** Stub
**Date:** 2026-05-07

## Scope

This follow-up spec owns the executable `pkg update` flow:

- HTTP fetch of `index.json` and `index.json.bundle`;
- ETag and 304 handling;
- re-evaluating cached `expires_at` on 304;
- `meta.json` persistence for `last_fetched`, `last_index_version`,
  `last_integrated_time`, and `consecutive_fetch_failures`;
- package-file downloads and hash verification;
- `db.sqlite` search/info cache schema and updates;
- command integration for `pkg update`, `pkg search`, and `pkg info`.

The distribution implementation plan creates the metadata, freshness,
rollback, trust-root, and verifier boundaries this flow will use, but it
does not implement network/cache persistence.
