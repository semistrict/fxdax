---
id: TASK-131.7
title: >-
  crash-dax-killed: a synced rewrite lost (rewritten page 1 byte 0 holds 0xae:
  no write since generation 407, the last synced, wrote it), at 7c9948612 with
  the VMM's coalesced reprotects
status: Done
assignee: []
created_date: '2026-10-29 22:24'
updated_date: '2026-10-30 00:28'
labels: []
dependencies: []
parent_task_id: TASK-131
priority: high
ordinal: 479000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r261, fxfs-guest-crash-dax-killed round 1 of 2 at 7c9948612 (unpushed: TASK-44.22's UNMAP fix, fxfs_dax_cow and grow_ahead KCSAN marks, and the VMM's uffd remap reprotecting in-place maps one after another in the window at once, deferred past installs after them). First such failure seen; crash-dax-killed passed at r250. Suspect: the coalescing reordering a reprotect after an install of overlapping memory in the same change (8e5b6c3cf flushes the run before any install). Test: 6 rounds each at a27e50fd8 (no VMM change, r263), 7c9948612 (r261), 8e5b6c3cf (r264).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The cause found, and crash-dax-killed passing 10 rounds with the VMM change in
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Cause: the test, not the store. In killed mode nothing quieted the writers before KILL_ME: a writer said its next rewrite after KILL_ME, which the host kills the VM on and doesn't read past, and wrote it, through DAX straight to the store. r261 round 1: page 1 held generation 423's byte, the write after 'rewriting 0 10 422', said after 'kill me' ((422+7) mod 16 = 1); r261-killed-7c99 round 2: page 10 held 310's, after 'rewriting 0 3 309'. Fix 9601cd2ba: report() reads quiet under stdout's lock, and the killed crash sets quiet and says KILL_ME under it. The VMM's coalesced reprotects (and 8e5b6c3cf's ordering) aren't involved. Verifying: crash-dax-killed x10 at 9601cd2ba (r261).

GCE r261 at 9601cd2ba: fxfs-guest-crash-dax-killed 10 rounds passed (it failed 2 of 3 rounds before).

Verified: macOS scripts/test.sh (clippy, 1827 workspace tests and the rest) at 641568faf; Linux workspace tests on GCE r263 at 2d95a4861 (main, which has the change) passed; scripts/upstream-changes.py --check passed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The crash-dax-killed failure was the test's: writers kept writing through DAX after the guest said KILL_ME, so a rewrite the host never heard of reached the store. Writers now go quiet under stdout's lock before KILL_ME is said under it (9601cd2ba, on main as 40fdf8e87). fxfs-guest-crash-dax-killed passed 10 of 10 rounds on GCE r261 (2 of 3 failed before); release suites passed 2 rounds on r263; workspace tests passed on macOS and Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
