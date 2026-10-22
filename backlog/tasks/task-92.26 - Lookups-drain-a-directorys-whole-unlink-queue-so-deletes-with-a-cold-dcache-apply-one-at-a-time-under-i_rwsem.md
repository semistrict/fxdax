---
id: TASK-92.26
title: >-
  Lookups drain a directory's whole unlink queue, so deletes with a cold dcache
  apply one at a time under i_rwsem
status: To Do
assignee: []
created_date: '2026-10-22 21:06'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 301000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
lookup drains its directory (fs.rs:694), and pending counts each unlink under its directory (249-255, 1244-1246), so with a cold dcache each unlinkat's lookup applies the previous unlink as a one-op transaction under the parent's lock, the synchronous pattern doc-6 removed. create, symlink, link and rename drain the same way. rm -rf, git clean or find -delete on a freshly booted fork's tree drop from about 180-230k to 55-100k ops/s. The bench warms dentries first (test_fs_bench.rs:1381, 1407), so it never measures this.

Where: guest/rust/fxfs-linux/src/fs.rs:249-255, guest/rust/fxfs-linux/src/fs.rs:694, guest/rust/fxfs-linux/src/fs.rs:828, guest/rust/fxfs-linux/src/fs.rs:861, guest/rust/fxfs-linux/src/fs.rs:922, guest/rust/fxfs-linux/src/fs.rs:988, guest/rust/fxfs-linux/src/fs.rs:1244-1246, guest/rust/fxfs-linux/src/fs.rs:1252-1254, guest/rust/fxfs-linux/src/fs.rs:1325-1327, guest/linux/fs/fxfs/super.c:245, guest/linux/fs/fxfs/super.c:632, guest/tests/test_cases/src/test_fs_bench.rs:1381, guest/tests/test_cases/src/test_fs_bench.rs:1407.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present. lookup drains its directory (guest/rust/fxfs-linux/src/fs.rs:693-694). DelayedOp::inos counts the dir for every unlink (fs.rs:249-255; unlink_later 1237-1245), so drain(&[dir]) applies queued unlinks under that dir (fs.rs:1325-1327). create/mknod (828), symlink (861), link (922), unlink (961) and rename (988) drain the same way. Not tracked. TASK-79.21 covers correctness of the same queue. Related: TASK-26.18.2, TASK-79.21.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [ ] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
