---
id: TASK-44.15
title: >-
  Random file operations with an oracle on the guest Fxfs: fsx-style mmap,
  write, truncate and fsync, differential against ext4, xfstests' generic group
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: medium
ordinal: 394000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The guest checks are each a scenario someone thought of. fsx's random mixes of mmap stores, writes, truncates and syncs, verifying every byte, and the same sequence run on ext4 to compare, find interleavings no one listed; xfstests' generic group is the kernel's own suite for such bugs. Under fault and delay injection they reach the DAX paths' rare branches.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A guest test runs seeded fsx-style operation sequences on Fxfs with and without DAX, verifying contents, and the same on ext4 to compare
- [ ] #2 xfstests' generic group runs against the guest Fxfs, its failures triaged and filed
- [ ] #3 Both run with injection on (the fault and delay injection tasks)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
