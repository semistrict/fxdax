---
id: TASK-149.12
title: Differential testing against ext4 in a guest
status: Done
assignee: []
created_date: '2026-11-01 06:13'
updated_date: '2026-11-08 04:32'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: low
ordinal: 513000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The same random operations on Fxfs and ext4, results compared, finds POSIX mistakes without an oracle written by hand, the class xfstests found in Fxfs one at a time.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The same seeded operations run on both in a guest
- [x] #2 Differences are reported, those allowed listed with a reason
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-06: built under TASK-44.15: fxfs-guest-fsx(-dax, -delay-dax, -faults-dax) run the same seeded wide-mix scripts on Fxfs and ext4 in one guest, step by step (guest/tests/test_cases/src/differential.rs); outcomes and bytes held to the model, which ext4 is held to too, and each step's mtime and ctime changes compared between the two, those allowed listed in differential::ALLOWED with why (none so far: GCE r693 and r694, about 400,000 steps, no difference). AC 1 and 2 met by that; left open here for the user to close with TASK-44.15.

2026-11-07 sweep: verified on main 2a5cd577f: guest/tests/test_cases/src/differential.rs runs the same seeded scripts on Fxfs and ext4 in one guest (fxfs-guest-fsx, -dax, -delay-dax, -faults-dax; 5fbed9c8d), differential::ALLOWED empty (differential.rs:27), its 7 unit tests in CI's test (guest) part, green in 42440067129 (land-c8); GCE r693, r694, r699 ~400,000 steps no difference; TASK-44.15 done (812f22a60). DoD: CI 42440067129's test (workspace) and macos (workspace) green; upstream-changes.py --check passes on main.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Done by TASK-44.15 (5fbed9c8d): fsx-style seeded scripts run on Fxfs and ext4 side by side in one guest, each step's outcome, bytes and mtime/ctime changes compared; allowed differences listed with why in differential::ALLOWED (none). Verified: differential.rs on main, its unit tests green in CI 42440067129, GCE r693/r694/r699 (~400,000 steps, no difference).
<!-- SECTION:FINAL_SUMMARY:END -->
