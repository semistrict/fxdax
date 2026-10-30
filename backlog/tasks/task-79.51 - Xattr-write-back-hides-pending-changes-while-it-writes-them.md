---
id: TASK-79.51
title: Xattr write-back hides pending changes while it writes them
status: Done
assignee: []
created_date: '2026-10-21 20:59'
updated_date: '2026-10-30 13:08'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 239000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
write_xattrs takes the pending map before committing it, so concurrent getxattr, listxattr and XATTR_CREATE/REPLACE see the old state. For example, overlayfs misreads opaque directories.

Where: guest/rust/fxfs-linux/src/fs.rs:1122, guest/rust/fxfs-linux/src/fs.rs:1131, guest/rust/fxfs-linux/src/fs.rs:1094, guest/rust/fxfs-linux/src/fs.rs:1107, guest/rust/fxfs-linux/src/fs.rs:1178.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-50.6.1, TASK-50.6. TASK-50.6.1 (Done) fixed a different overlayfs defect: a removed directory's object tombstoned too early. No task covers write_xattrs taking the pending map before it commits.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test reproduces the failure on the code before the fix
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
write_xattrs leaves each change pending until written, removed only if unchanged since. xattrs_read_as_written_back: r283 without the fix, 'turn 5: getxattr: No data available'; with it passes (r273).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
write_xattrs leaves each change pending until it is written (main 36adb1ce4). xattrs_read_as_written_back fails without the fix (r283, 'turn 5: getxattr: No data available') and passes with it (r273, r286). The test is a guest check, on Linux only; the crate's workspace tests pass on macOS and r285.
<!-- SECTION:FINAL_SUMMARY:END -->
