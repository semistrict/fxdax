---
id: TASK-49
title: >-
  StatStore: a store's bytes split into its own and those it shares with its
  parents
status: Done
assignee: []
created_date: '2026-10-19 11:13'
updated_date: '2026-10-19 12:49'
labels: []
dependencies: []
priority: high
ordinal: 148000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Per-tenant storage billing needs what each store costs alone. StatStoreResponse has capacity, used and available: used counts every extent the store holds, including those a fork inherited from its parents and still shares with them, so forks of one base are billed for the base's bytes again and again.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 StatStore reports, besides used, the bytes of extents only the store holds and the bytes of those it shares with the branches it was forked from
- [x] #2 Both are exact for a base, a fork, and a fork of a fork, as extents are written, freed and inherited: a test checks each
- [x] #3 The split costs StatStore no read of the store's extents' data, and stays correct after the daemon restarts
- [x] #4 fxdaxctl stat prints both
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Store keeps own (bytes of extents whose ExtentId.branch is its own) beside stored, summed from the index at open and kept by finish_write/free/adopt; inherited = stored - own: what the store holds of its parents' extents, whether or not a parent still holds them. No extent data is read. Verified: scripts/test.sh on macOS (2096 workspace tests plus the Shuttle suites); Linux on GCE b3 (2152 workspace tests and every suite; guest cases 18/18, fxfs-guest-suspend-dax passing 3/3 on rerun after one vsock harness flake); scripts/upstream-changes.py --check passes. No upstream code changed. Test: extent_store tests/store.rs stores_tell_their_own_bytes_from_those_inherited (base, fork, fork of fork; writes, frees, reopen); fxdax tests/control.rs and tests/fxdaxctl.rs check StatStore and fxdaxctl stat.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
StatStoreResponse gains own and inherited (fields 7, 8): a store's bytes in extents it wrote, and in those it holds of the branches it was forked from, kept from the index without reading data and exact across reopen. fxdaxctl stat DIR prints them. Commit 275683e.
<!-- SECTION:FINAL_SUMMARY:END -->
