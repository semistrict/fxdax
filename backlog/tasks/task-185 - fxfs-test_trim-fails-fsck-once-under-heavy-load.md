---
id: TASK-185
title: fxfs test_trim fails fsck once under heavy load
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 15:19'
updated_date: '2026-11-05 19:10'
labels:
  - fxfs
  - flaky
dependencies: []
priority: low
type: bug
ordinal: 640000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen once on macOS (collector-fixes, 2026-11-05, load average 53, in FXDAX_TEST_PARTS='workspace shuttle' scripts/test.sh): fxfs object_store::data_object_handle::tests::test_trim failed after 70 s: 'fsck_with_options failed: Fsck encountered 1 errors, 0 warnings'. Alone it passes 3 of 3 (8 s). fxfs doesn't depend on extent_remote, which the branch changed. Find the fsck error (run under stress with load, keep fsck's output) and whether trim races something.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 #1 The fsck error test_trim hit is named and reproduced
- [x] #2 #2 Fixed, with a test red before
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Find the failing run's log and which fsck failed.
2. Check whether that run's code had TASK-176's fix.
3. Red: test_trim with its fs2 and remount checks unlocked, 300 under load. Green: land-e as is, 500 under load.
4. Verify the fxfs, fxfs-posix and fxdax suites and the macOS workspace.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Cause
The failing run (worktree t3-35008d14, collector-fixes at bc26b2357, log /tmp/fxdax-doc/mac.log) was based on 6cb5e90b5, before TASK-176's fix (254177bba). Neither fa9dc7036 nor 254177bba is in bc26b2357's history. Its panic, data_object_handle.rs:5206:64, is that file's fsck of fs2 (the filesystem replayed from the hook's snapshot) with `options`, no_lock: true. Its error, ExtraAllocations of one owner-3 extent (the root parent store, which holds the journal), is one of TASK-176's. The journal.rs panic after it is the compaction left in flight, as in TASK-176. land-e and main have the fix: nothing else is left.

## Runs (macOS, 2026-11-05, debug build, 10 at once, load average 28 to 75: TASK-184's stress, 16 busy loops)
- Red: land-e with fs2's and the remount's checks unlocked again (`(options(true), options(true))`): 18 of 300 failed, all at fs2's fsck (line 5217): ExtraAllocations and MissingAllocation of owner 3, OrphanedObject(4, _), AllocatedBytesMismatch.
- Green: land-e as is: 500 of 500.

## Blind spot and structure
TASK-176's notes cover it: a check without the lock of a filesystem whose own work goes on. The filing agent took the failing branch to have the fix because main did. No code change here.

## Verified (macOS, 2026-11-05)
cargo nextest run -p fxfs -p fxfs-posix -p fxdax: 1126 of 1126; FXDAX_TEST_PARTS=workspace scripts/test.sh --no-fail-fast: 2133 of 2133; upstream-changes.py --check passes. GCE (Linux) not run.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
No new bug: the failing run's branch (collector-fixes, bc26b2357, based on 6cb5e90b5) lacked TASK-176's fix (254177bba), and its failure was fs2's unlocked fsck (line 5206) finding an extra owner-3 allocation, TASK-176's signature. Red: land-e with fs2's and the remount's checks unlocked again, 18 of 300 under load, all at fs2's fsck. Green: land-e as is, 500 of 500 under load (10 at once, load 28 to 75). macOS package suites and workspace green; GCE not run.
<!-- SECTION:FINAL_SUMMARY:END -->
