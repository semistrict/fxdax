---
id: TASK-149.20.1
title: xfstests runs in no suite
status: Done
assignee:
  - '@claude'
created_date: '2026-11-07 13:36'
updated_date: '2026-11-07 20:42'
labels:
  - testing
dependencies: []
parent_task_id: TASK-149.20
priority: medium
type: task
ordinal: 666000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-149.20. xfstests-fxfs-guest and xfstests-fxfs-guest-dax (guest/tests/test_cases/src/test_xfstests.rs) are tagged Manual: no script's suite, nor the nightly, runs them, so the kernel's own filesystem suite runs only when someone names it, and its KNOWN_FAILURES list (now failing the case when a listed test passes, TASK-149.20) is checked only then. Last run: TASK-150.21's, GCE 2026-11 (16 of 650, the known list).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 xfstests (both cases) runs on a schedule (the nightly, or a weekly run of its own), its results kept as the guest suites' are
- [x] #2 A run on main with the known list as it is passes, or what fails is filed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Tag both xfstests cases nightly (no weekly suite exists); Manual's doc no longer names them; nightly.yml says so.
2. Time both on GCE (r960) against the nightly's budget, with the known list as it is.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done (commit 25391f205). No weekly suite exists, so both cases are tagged nightly. They take 12-16 min each on GCE (r960: 865 s and 944 s; r963: 736 s and 792 s) against the nightly's 8 h. The first run (r960, main d7a36088b) failed both: generic/303, 525, 372 and 401, listed, passed with DAX and without. Taken out; r963 passed both, failing the 12 listed alone. forked-perf's r942 saw the same four. 401's TASK-144 was fixed by 56fdf6b12 (closed). 372 is one of TASK-143's four; 414, 578 and 702 still fail, so it stays open. 303 and 525 cited no task. The nightly itself still waits on its GitHub credentials (OPEN_QUESTIONS), so it doesn't run yet; when it does, xfstests runs with it.

CI 42410439072 on land-r2 4ab1eb0f2: all 15 jobs green, macOS and Linux workspace, guest, guest-sim and its sweep, and clippy scripts among them.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
xfstests-fxfs-guest and -dax are in the nightly suite (12-16 min each). The known list was corrected: generic/303, 525, 372 and 401 now pass. r963 passed both with the 12 listed alone failing.
<!-- SECTION:FINAL_SUMMARY:END -->
