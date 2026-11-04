---
id: TASK-92.38
title: >-
  The followed bench builds both followers' root images while the writer's first
  iteration runs
status: Done
assignee: []
created_date: '2026-10-22 21:06'
updated_date: '2026-11-04 10:39'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 313000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Followers are spawned one after the other, only once the writer is ready (test_fs_bench.rs:439-458), while the writer starts iteration 1 at once (1693-1699). Each spawn runs an unpinned mkfs.ext4 -d and fsyncs hundreds of MB onto the writer's host filesystem (vm.rs:218-231, 272-288, 443-463). Iteration 1 is then charged to following: seq-write is 58.1 MiB/s against 65.6 alone.

Where: guest/tests/test_cases/src/test_fs_bench.rs:439-458, guest/tests/test_cases/src/test_fs_bench.rs:1693-1699, guest/tests/test_cases/src/vm.rs:218-231, guest/tests/test_cases/src/vm.rs:272-288, guest/tests/test_cases/src/vm.rs:443-463.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present; the bench's followed code is unchanged since 6a0a3f8. The writer is spawned and, once it says ready, the followers are spawned one after the other (guest/tests/test_cases/src/test_fs_bench.rs:443-464). Each builds its own {role}.rootfs.ext4 at spawn (root_image at 495; vm.rs spawn_api 218 -> mkfs.ext4 ~282). The writer sends ready() right after mount and sync and starts its iterations (test_fs_bench.rs:1758-1766). Not tracked. Related: TASK-42.7, TASK-79.32.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [ ] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A followed bench's writer waits for both followers to be spawned, their root images built, before its first iteration. Verified: macOS scripts/test.sh (workspace 2,083/2,083, Shuttle, scripts); GitHub CI on the branch (Linux workspace, guest-sim 3,000 seeds); GCE r539 at 8da75b3cc with Firecracker a2db2b3b0 (fxfs-guest-dax, checks-dax, -delay, -small, script-full-dax, xfstests-fxfs-guest-dax, checks, crash-dax, pgbench-dax all pass).

Not measured: decided the handshake stands on its own, the overlap it removes being plain from the code.
<!-- SECTION:FINAL_SUMMARY:END -->
