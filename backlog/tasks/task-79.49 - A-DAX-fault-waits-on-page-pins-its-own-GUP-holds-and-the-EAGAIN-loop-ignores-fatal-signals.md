---
id: TASK-79.49
title: >-
  A DAX fault waits on page pins its own GUP holds, and the -EAGAIN loop ignores
  fatal signals
status: In Progress
assignee:
  - '@main-session'
created_date: '2026-10-21 20:59'
updated_date: '2026-10-24 15:11'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 237000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
drop_slice in a fault waits in dax_break_layout for pages the same GUP call pinned. make_room discards -ERESTARTSYS, so a GUP that fills the window spins and cannot be killed.

Where: guest/linux/fs/fxfs/dax.c:1863, guest/linux/fs/fxfs/dax.c:1368, guest/linux/fs/fxfs/dax.c:1870, guest/linux/fs/fxfs/dax.c:1231.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-26.15, TASK-39.6, TASK-26.18.8. No task covers dax_break_layout in fault context or the -EAGAIN loop ignoring fatal signals. TASK-39.6 ported dax_break_layout to 6.18. TASK-26.15 added the window's make-room LRU.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test reproduces the failure on the code before the fix
- [ ] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
The fault loop now gives up on a fatal signal (VM_FAULT_SIGBUS), and a fault's slice drop no longer waits for pinned pages (fxfs_dax_drop_pinned with pinned=false returns -EBUSY), so a GUP pinning its own pages fails instead of sleeping. dax_break_layout's wait is interruptible already. No deterministic seam for the self-pin (an O_DIRECT read into a mapping of the file whose slice must drop mid-GUP).

Fix landed in 075723b80; no deterministic test (see the notes above). GCE r65, 2026-10-24, batch on main+round 2b: Linux workspace 2620 passed, Firecracker extent tests, and 17 guest cases (checks, crash, fxfs-guest, follow, follow-torture, snapshot with and without DAX; extent-guest, live-fork, daemon-lost, pgbench both) green on vmlinux-b3/b4; macOS scripts/test.sh 2508 passed.
<!-- SECTION:NOTES:END -->
