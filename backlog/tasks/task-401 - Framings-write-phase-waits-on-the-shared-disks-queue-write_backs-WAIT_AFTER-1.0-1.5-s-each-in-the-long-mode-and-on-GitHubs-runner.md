---
id: TASK-401
title: >-
  Framings' write phase waits on the shared disk's queue: write_back's
  WAIT_AFTER 1.0-1.5 s each in the long mode and on GitHub's runner
status: To Do
assignee: []
created_date: '2026-11-13 15:12'
labels:
  - perf
  - extentd
dependencies: []
priority: high
type: bug
ordinal: 784000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-336's instrumentation. With the framer's CPU work cut (TASK-336), the framings still past 1 s on GitHub's 4-CPU runner (sandboxes short mode, run 43435575870 on main 487f443a8: 1 a host, 1,085 and 1,549 ms) and many of ubu1's long mode's (c6ba0d117: 1.0-1.5 s) are their write phase with almost no runnable time: segments::write_back writes each MiB of a copy, or an in-place framing's tail, and waits for it to reach the disk (sync_file_range WAIT_BEFORE|WRITE|WAIT_AFTER, TASK-165.9) behind a queue the VMs, the flushes and RustFS keep full; one of 1,532 ms wrote a tail of a few KiB. Waiting paced framings so that commits' syncs didn't wait behind a framed segment's 32 MiB (GCE r728), when every segment was framed in a copy; now most are framed in place (tail only) and copies are ~6% of the raw bytes (short mode: framed 0.26-0.33 GB against raw 5.0 GB). Ideas, to measure against durable lag (doc-12) and the framing check: write a tail without waiting (the making durable after syncs it); begin a copy's write-back without waiting and let segments::make_durable wait; an I/O priority for framings' writes below flushes'.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 GitHub's sandboxes short mode: no framing's write phase past 1 s over several runs, durable lag no worse
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
