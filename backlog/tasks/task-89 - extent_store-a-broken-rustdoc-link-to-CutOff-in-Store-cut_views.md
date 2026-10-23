---
id: TASK-89
title: 'extent_store: a broken rustdoc link to CutOff in Store::cut_views'
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-22 13:26'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
priority: low
type: chore
ordinal: 263000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
cargo doc --document-private-items warns: Store::cut_views links CutOff, which isn't in scope there. Seen on main and on integrate-2.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 cargo doc -p extent_store --document-private-items gives no warning
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Store::cut_views's rustdoc link to CutOff now uses its crate path, crate::CutOff, because CutOff is not in scope in store.rs. On main at 47b7a348, cargo doc -p extent_store --document-private-items, with and without --no-deps, gives no warning for extent_store; only dependency crates warn. The workspace suites pass on macOS and on Linux (GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
