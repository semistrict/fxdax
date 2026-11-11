---
id: TASK-362
title: >-
  extent_store::crash recovers_from_its_bucket_with_and_without_its_disk: round
  81 opened at epoch 10 with another epoch's extents
status: To Do
assignee: []
created_date: '2026-11-11 16:21'
labels: []
dependencies: []
priority: high
ordinal: 744000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ubu1, 2026-11-11, tree 1e5367ef (cargo nextest run -p extent_store -p extent_remote, debug): crates/extent_store/tests/crash/kept.rs:318 'round 81: at epoch 10': left [(2, 2, 14748), (3, 8, 7992), (4, 3, 32006), (5, 7, 34132)] root "root 10", right [(3, 8, 7992), (4, 3, 32006), (7, 2, 31129), (8, 1, 22419)] root "root 10". The root matches epoch 10 but the extents don't: a store recovered from the bucket and its disk holding a mix. Not the TASK-359 check (MemDir, no host path). Possibly the same family as TASK-341 (openings_cut_at_each_write_open_again: opened at epoch 22 holding epoch 21's contents). Log: ubu1:/tmp/t358-a.log.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The failing round reproduced by its seed, its cause found and fixed, with a test red before the fix
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
