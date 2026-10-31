---
id: TASK-79.23
title: >-
  A failed follower batch leaves a half-applied tree serving readers, lets go of
  its commit, and drifts from its writer for good
status: Done
assignee:
  - '@claude'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-31 02:23'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 211000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On an apply error the barrier is released into a half-applied state, settle is skipped and Previous is let go of. The journal reader has already moved past the batch, so the rest of it is never applied. A fetch error lets go of Previous while readers still read the old trees. Only ESTALE leads to a remount.

Where: crates/follower/src/lib.rs:157, crates/follower/src/lib.rs:162, crates/follower/src/lib.rs:168, crates/fxfs/src/follower.rs:251, crates/fxfs/src/follower.rs:273, crates/fxfs/src/object_store/journal.rs:1304, guest/linux/fs/fxfs/super.c:2083.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-42.4, TASK-42.6, TASK-42.10. No task covers error handling of a failed follower batch: half-applied tree, Previous let go, journal reader not rewound, no remount. TASK-42.4 (Done) built apply, fetch and Previous. TASK-42.10 added the barrier and settle.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test reproduces the failure on the code before the fix
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Uncommitted in worktree batch8 (2026-10-30), awaiting a GCE run: gcloud needs reauthentication, r289 stopped at it. crates/follower: a batch failing at fetch or apply records it under the barrier (Kernel::fail); fill() returns Failed, batches fail at once after; rule 'fail' with Shuttle needs_failing and failing_exhaustively. crates/fxfs follower.rs: failed flag, what keeps the previous commit stays to unmount, no advance after. Guest: readers ESTALE (Platform::filling), remount recovers; attr read errors now fail the batch rather than settle an inode gone. Tests: follower_sim failure.rs (mutants killed), Shuttle 10 of 10, fxfs-guest-follow-remount(-dax) NOT YET RUN.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A batch that fails ends the follower: readers get ESTALE until it remounts, and the commit it was as of stays held. Shuttle's needs_failing and failing_exhaustively, follower_sim's failure module, fxfs-guest-follow-remount[-dax] (r293, 3 rounds each). Linux workspace 1986/1986 and the Shuttle suites on GCE r292, macOS scripts/test.sh 1852 and the rest, scripts/upstream-changes.py --check.
<!-- SECTION:FINAL_SUMMARY:END -->
