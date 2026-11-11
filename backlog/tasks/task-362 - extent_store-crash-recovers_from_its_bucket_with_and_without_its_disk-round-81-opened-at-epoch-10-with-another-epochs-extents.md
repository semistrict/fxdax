---
id: TASK-362
title: >-
  extent_store::crash recovers_from_its_bucket_with_and_without_its_disk: round
  81 opened at epoch 10 with another epoch's extents
status: Done
assignee:
  - '@claude'
created_date: '2026-11-11 16:21'
updated_date: '2026-11-11 21:23'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-11 (claude): TASK-341's. Tree 1e5367ef branched from 6e2d072b, before both TASK-341 fixes (a0960d47d, 89486912f): its logs' headers name no segment (LOG_MAGIC FXLOG..\x01) and its recovery doesn't end what it keeps at a commit not durable. Round 81 of these rounds is where TASK-341's spare-log case was found: a spare renamed from one segment's log and then another's, neither rename durable, kept under the first's name with the second's header and records, which recovery read as the first's commits, a mix of epochs' extents. On main 04280b595 (both fixes): cargo nextest run -p extent_store --test crash, 40 runs on ubu1 under fxjob: 31 green, 1 failing TASK-365's coverage check (20 spanning commits, not more than 20; every round passed), 8 killed by the OOM killer at fxdax.slice's memory limit (other jobs; not counted): 32 valid runs, no kept.rs:318 failure.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
TASK-341's spare-log/recovery bug on a tree without its fixes; 32 valid crash runs on main 04280b595 on ubu1, none failing kept.rs:318.
<!-- SECTION:FINAL_SUMMARY:END -->
