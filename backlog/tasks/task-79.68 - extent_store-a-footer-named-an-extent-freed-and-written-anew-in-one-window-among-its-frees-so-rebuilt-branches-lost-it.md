---
id: TASK-79.68
title: >-
  extent_store: a footer named an extent freed and written anew in one window
  among its frees, so rebuilt branches lost it
status: Done
assignee: []
created_date: '2026-10-23 03:10'
updated_date: '2026-10-24 16:13'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 319000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found while fixing TASK-79.1 (round 1). An extent freed while open, then written again before its commit, was named by its segment's footer both among the extents the commit sealed and among those it freed. Readers of footers apply the frees after the extents (Store::rebuild, and recovery in object storage), so the branch they rebuilt lacked the extent. Footers uploaded before the fix by stores that did this still name such extents (no migration).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A footer never names among its frees an extent its window's last commit holds
- [x] #2 Store::rebuild and object-storage recovery of such a window keep the extent, pinned by tests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Finalization check (after the push of 47b7a348): not done. AC#2: there is no test that object-storage recovery of a window with an extent freed and rewritten keeps the extent. extent_remote has no such test. Its simulation builds its own footers (sim/world.rs), not through extent_store's Window, and no extent_remote test frees an extent. Only Store::rebuild is pinned, by footers_hold_extents_freed_open_and_written_anew. What's needed: an extent_remote test that uploads such a window's segment, as extent_store now writes its footer, recovers the branch from object storage, and finds the extent. Audit: The claimed AC#1 and DoD items hold. 0a9e9547 (same patch-id as the reported 7ec1bb41) is on main, and Window::commit removes each extent the commit touched from freed (segments.rs:807). footers_hold_extents_freed_open_and_written_anew (tests/store.rs:2366) failed before (rebuild gave '[]') and passes on Linux (:264) and macOS (:584). The not-done verdict is correct. AC#2 also asks for an object-storage recovery test, and there is none. rg finds no .free( of an extent in any extent_remote test, nor any remote test of an extent freed and written anew. No commit since 8b505f9f touches extent_remote for this, and the round-1 report says 'no extent_remote change was needed'. What is needed: an extent_remote test that recovers a branch from object storage after a window that freed and rewrote an extent, with the footer written as extent_store now writes it, and finds the extent.

Handed over (2026-10-24): fixed on main (a footer names no extent freed and written anew among its frees). Left: its criterion for object-storage recovery is not pinned by a test of its own.

remote::tests::extents_freed_and_written_anew_survive_recovery passes on Linux (GCE r65) and macOS. Linux: GCE r65 (2026-10-24, us-east1-b) workspace 2621/2621 passed, guest checks, checks-dax and pgbench-dax OK on kernel b4 (main's guest code); macOS: scripts/test.sh green (2508 workspace tests, Shuttle and scripts suites), scripts/tests 84 OK; scripts/upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A footer no longer names among its frees an extent its window's last commit holds; Store::rebuild and recovery from object storage keep such an extent, pinned by tests (09d071c0a), green on macOS and Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
