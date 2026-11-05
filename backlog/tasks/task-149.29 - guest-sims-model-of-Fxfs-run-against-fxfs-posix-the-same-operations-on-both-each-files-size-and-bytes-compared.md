---
id: TASK-149.29
title: >-
  guest/sim's model of Fxfs run against fxfs-posix: the same operations on both,
  each file's size and bytes compared
status: To Do
assignee: []
created_date: '2026-11-05 06:37'
labels:
  - guest-sim
dependencies: []
parent_task_id: TASK-149
priority: medium
type: task
ordinal: 635000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
guest/sim tests dax.c against a second writing of Fxfs (guest/sim/fxfs/fxfs.c), and where the model differs from Fxfs a run proves nothing or fails a correct dax.c. Nothing checks the model against Fxfs: its drifts show only as sim failures that take hours to reduce. Two so far: a seal's minted extents called open past the COMMIT (interleave -C seed 32826, README's simulator bugs), and growths that kept what a cut left past the end, where Fxfs zeroes it (TASK-149.14.2, failures 140272), which fxfs-posix's every_growth_finds_what_a_cut_left_past_the_end_zeroed already said of Fxfs. A differential run (the model's fxfs.h calls, write_many, write_inode with and without FXFS_SET_KEEP_TAIL, clone_range, punch, fsync, against fxfs-posix's Fs of the same operations) would find a drift as it is made, before a seed meets it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Random sequences of the operations dax.c and data.c call of Fxfs run on the model and on fxfs-posix, and each file's size and bytes up to it agree after every operation
- [ ] #2 With task-149.14.2-growth-keeps-tail or task-149.14.2-partial-write-keeps-past-end in, it fails
- [ ] #3 It runs in CI
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
