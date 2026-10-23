---
id: TASK-83
title: >-
  extent_store: a commit doesn't check the store failed after waiting at the
  write gate
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-22 13:26'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
priority: medium
type: bug
ordinal: 257000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found while restructuring commit_locked (integrate-2, extent_store's commit as phases): commit_locked waits at the gate for the writes in progress, and doesn't check `failed` again afterwards. A write or flush that fails the store meanwhile leaves the commit going on over a store whose state may differ from its files.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test fails the store while a commit waits at the gate, and the commit fails then
- [x] #2 Fixed; the extent_store suites, its Shuttle suites included, pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A commit that waited at the gate for writes in progress let the state's lock go and then went on without looking again. So it could commit over a store that a failing flush had failed meanwhile, or over a branch closed meanwhile. Having waited, a commit now rechecks the state as writable() does. A closing between commits fails on a failed store and closes nothing on a closed branch. Writes and cleaning's copies waiting at the gate check both too (e50e9f1a). New Shuttle scenarios fail the store, or close the branch, while a commit waits: the needs_rechecks_after_waiting_* tests show the commit went on without the rule, and with it the commit fails. The extent_store suites, Shuttle ones included, pass on macOS and Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
