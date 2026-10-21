---
id: TASK-79.21
title: The deferred unlink queue lets the kernel's view diverge from Fxfs
status: To Do
assignee: []
created_date: '2026-10-21 20:58'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 209000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Queued unlinks have no deadline: freed space stays held (ENOSPC), and later operations reach the journal first, so crash recovery is not a prefix. lookup and RENAME_EXCHANGE read nlink without draining the inode, so the kernel's count stays one high and the object's space is held until remount, or a linked object is reaped. After a failed batch, evict tombstones an object that a name still links.

Where: guest/rust/fxfs-linux/src/fs.rs:1252, guest/rust/fxfs-linux/src/fs.rs:1295, guest/rust/fxfs-linux/src/fs.rs:1221, guest/rust/fxfs-linux/src/fs.rs:1606, guest/rust/fxfs-linux/src/fs.rs:1019, guest/rust/fxfs-linux/src/fs.rs:694, guest/rust/fxfs-linux/src/fs.rs:1363, guest/rust/fxfs-linux/src/fs.rs:1242.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-26.18.2, TASK-26.19.9, TASK-50.6.1. Not tracked. TASK-26.18.2 (Done) introduced the deferred unlink queue. TASK-26.19.9 drains it in fh_to_dentry. TASK-50.6.1 (Done) fixed a different tombstone-timing bug, for removed directories.
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
