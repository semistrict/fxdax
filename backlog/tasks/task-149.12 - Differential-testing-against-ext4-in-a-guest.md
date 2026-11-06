---
id: TASK-149.12
title: Differential testing against ext4 in a guest
status: To Do
assignee: []
created_date: '2026-11-01 06:13'
updated_date: '2026-11-06 09:16'
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
- [ ] #1 The same seeded operations run on both in a guest
- [ ] #2 Differences are reported, those allowed listed with a reason
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-06: built under TASK-44.15: fxfs-guest-fsx(-dax, -delay-dax, -faults-dax) run the same seeded wide-mix scripts on Fxfs and ext4 in one guest, step by step (guest/tests/test_cases/src/differential.rs); outcomes and bytes held to the model, which ext4 is held to too, and each step's mtime and ctime changes compared between the two, those allowed listed in differential::ALLOWED with why (none so far: GCE r693 and r694, about 400,000 steps, no difference). AC 1 and 2 met by that; left open here for the user to close with TASK-44.15.
<!-- SECTION:NOTES:END -->
