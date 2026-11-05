---
id: TASK-149.30
title: >-
  Neither guest/sim nor a guest suite finds a resize that moves a DAX file's
  last block from under the window
status: To Do
assignee: []
created_date: '2026-11-05 15:01'
updated_date: '2026-11-05 15:58'
labels:
  - testing
dependencies: []
parent_task_id: TASK-149
ordinal: 639000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-160.32 ran every hand mutant against its killer. task-150.9-resize-moves-the-last-block (guest/mutants), data.c's resize without FXFS_SET_KEEP_TAIL, so that Fxfs zeroes a DAX file's last block past the end by moving it from under the window, is killed by nothing since copying on write: fxfs-guest-checks-dax's append_stopped_short_in_a_page killed it on 2026-11-03 (GCE fxdax-test-t18-ac), and on 2026-11-05 passed with it in, alone in 3 rounds and with the suite's every check (GCE fxdax-test-r625); guest/sim fails no one-run test, scenario, or 300 seeds of interleave and failures with it in, though its model of Fxfs moves the block so (zero_tail, fxfs.c). The bug is real (TASK-150.9), so this is a blind spot of both: a window page reading the old block until it is freed, a model freeing it later than Fxfs, or no check reading the block after the flush that frees it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The reason no simulator run fails with task-150.9-resize-moves-the-last-block in is recorded
- [ ] #2 A simulator run fails with it in, and its header names that run as its killer, Kernel: sim
- [ ] #3 The run passes without it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
