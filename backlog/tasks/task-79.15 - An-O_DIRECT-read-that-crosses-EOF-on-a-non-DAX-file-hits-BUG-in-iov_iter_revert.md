---
id: TASK-79.15
title: >-
  An O_DIRECT read that crosses EOF on a non-DAX file hits BUG() in
  iov_iter_revert
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
ordinal: 203000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
direct_IO truncates the iterator to the bytes before EOF and never re-expands it, so generic_file_read_iter reverts past the iterator's start: BUG() on ubuf iterators. An unprivileged 4 KiB O_DIRECT read of a 100-byte file oopses the guest.

Where: guest/linux/fs/fxfs/super.c:1453, guest/linux/fs/fxfs/super.c:1456, guest/linux/fs/fxfs/super.c:1489.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-26.19.11. Not tracked. TASK-26.19.11 (Done) added direct_IO and a direct_io check of reads past EOF, which misses the ubuf iov_iter_revert BUG.
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
direct_read_past_end oopsed the kernel before (kernel BUG at lib/iov_iter.c:623, iov_iter_revert) and passes with 075723b80. GCE r65, 2026-10-24, batch on main+round 2b: Linux workspace 2620 passed, Firecracker extent tests, and 17 guest cases (checks, crash, fxfs-guest, follow, follow-torture, snapshot with and without DAX; extent-guest, live-fork, daemon-lost, pgbench both) green on vmlinux-b3/b4; macOS scripts/test.sh 2508 passed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
direct_IO re-expands the iterator it cut at EOF, so the caller's revert stays within it; direct_read_past_end red (BUG) then green on GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
