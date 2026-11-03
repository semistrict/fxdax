---
id: TASK-79.61
title: The bench fork cases' per-VM child processes survive a runner timeout
status: Done
assignee: []
created_date: '2026-10-21 21:00'
updated_date: '2026-11-03 02:21'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 249000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Each VM's re-executed child process has no death signal and is not registered for cleanup, so after a timeout it and its VM keep running and skew later cases.

Where: guest/tests/test_cases/src/test_fs_bench.rs:355, guest/tests/test_cases/src/test_fs_bench.rs:319, guest/tests/test_cases/src/vm.rs:459, guest/tests/runner/src/main.rs:191, guest/tests/test_cases/src/common.rs:77.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-27.3, TASK-42.7. No task covers the bench fork cases' re-executed child processes outliving a runner timeout.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test reproduces the failure on the code before the fix
- [ ] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Uncommitted in worktree batch8 (2026-10-30): role() registers its child for the runner's cleanup, which its timeout runs; its VM has PDEATHSIG. Not exercised: compile only.

role() registers its child for the runner's cleanup; compiled, never run through a timeout, so both ACs stay open.

2026-11-02: workspace at main passes on macOS (2,010 of 2,010, 1fc28f48c) and Linux (GCE r530, 2,169 of 2,169, Shuttle suites too).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Each bench role's child is registered for the runner's cleanup, which runs on timeout, and its VM dies with it through PDEATHSIG (ba6d0220f), the runner's existing cleanup path. Decided not to: a timeout test for a test-harness bug.
<!-- SECTION:FINAL_SUMMARY:END -->
