---
id: TASK-79.14
title: A short copy into a folio that was never read zeroes the file's existing data
status: Done
assignee:
  - '@main-session'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-24 15:11'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 202000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
write_begin skips reading a fully covered folio, and write_end (copied from ramfs's simple_write_end) zeroes the part not copied, marks the folio uptodate and dirty, and commits. A partly faulting source buffer, or copied==0 with a fatal signal, replaces the file's existing data with zeros on disk.

Where: guest/linux/fs/fxfs/super.c:1324, guest/linux/fs/fxfs/super.c:1349, guest/linux/fs/fxfs/super.c:1356.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-26.19.11. No task covers write_end zeroing an unread folio on a short copy. TASK-26.19.11 (Done) last changed write_begin's full-cover test, for large folios.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test reproduces the failure on the code before the fix
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
short_copy failed on the kernel before (vmlinux-evac2: the bytes past the copy were zeros) and passes with 075723b80. GCE r65, 2026-10-24, batch on main+round 2b: Linux workspace 2620 passed, Firecracker extent tests, and 17 guest cases (checks, crash, fxfs-guest, follow, follow-torture, snapshot with and without DAX; extent-guest, live-fork, daemon-lost, pgbench both) green on vmlinux-b3/b4; macOS scripts/test.sh 2508 passed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
write_end commits nothing of a short copy into a folio it never read, so the file's data around it stays; short_copy red then green on GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
