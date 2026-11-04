---
id: TASK-150.11
title: fxfs_dax_new_slice lacks the TASK-44.21 guard
status: Done
assignee: []
created_date: '2026-11-01 16:52'
updated_date: '2026-11-04 07:06'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: medium
type: bug
ordinal: 526000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit: the missing guard is traced, the interleaving only plausible. dax.c:2411 and :2437; the guard is at fxfs_dax_move, :2344. A sealed piece of a still-writable region can reach fxfs_dax_reach outside the gate and send a writable MAP, racing a seal that shrinks the region: a MAP into space given back.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test fails on the code as it stands, or shows the finding false or unreachable, which closes this as not a bug
- [ ] #2 Fixed, the test red again with the fix taken out
- [ ] #3 The notes name the blind spot, the test of its class, and what a search around it found
- [ ] #4 Passes on GCE in the guest suites, DAX and not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
guest/sim's candidate 4 (TASK-149.14, branch test/sim-next) looks the same: a writable MAP outside the gate, for a sealed piece of a writable region after a reflink. tests/sealed_piece_reach fails on dax.c with one task; seed 13464 without its fix; guest/sim/fixes/sealed-reach-locked.patch is its candidate fix. Two independent findings. The guest check sealed_piece_reach fails only on a lockdep kernel.

Confirmed on a real kernel: r423 xfstests-fxfs-guest-dax generic/013: 'mapped pages to 77 ..., writably (reach), past its region's 16 pages', WARN at dax.c:2132, stack fxfs_dax_reach < fxfs_dax_move, a path past TASK-44.21's guard, not fxfs_dax_new_slice as the audit said. TASK-44.21's fix left this gap.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Confirmed by r423 through fxfs_dax_move; fixed in e59d8621f, then made impossible by 7caa43578, which keeps shared blocks out of regions; open_extent_read_only kills the mutant task-150.11-looked-before-locked.
<!-- SECTION:FINAL_SUMMARY:END -->
