---
id: TASK-92.27
title: >-
  fsync and sync drain until the queue is empty, chasing unlinks queued after
  they started
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
ordinal: 302000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
drain_all runs until pending is empty, with no target taken at entry (fs.rs:1330-1332, 1336-1353), and every fsync and sync runs it first (1588-1603; super.c:1526). Workers stop below 32 queued ops (1295). So a database fsyncing during another process's warm bulk delete applies that process's unlinks until it pauses: hundreds of ms instead of about 300 µs. Per the verifiers, per-directory drains cannot chase.

Where: guest/rust/fxfs-linux/src/fs.rs:1255, guest/rust/fxfs-linux/src/fs.rs:1295, guest/rust/fxfs-linux/src/fs.rs:1330-1332, guest/rust/fxfs-linux/src/fs.rs:1336-1353, guest/rust/fxfs-linux/src/fs.rs:1588-1591, guest/rust/fxfs-linux/src/fs.rs:1598-1603, guest/linux/fs/fxfs/super.c:1526.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present. drain_all is apply_until(pending.is_empty()) with no target taken at entry (guest/rust/fxfs-linux/src/fs.rs:1330-1332, loop 1336-1352). sync and fsync both run it first (fs.rs:1595-1610; super.c fxfs_vfs_fsync at 1514). Workers stop once fewer than DELAYED_BATCH (32) ops are queued (fs.rs:1295). Not tracked. Related: TASK-26.18.2, TASK-79.21.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [x] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A drain applies what was queued before it began and returns, not chasing unlinks queued after (a_drain_leaves_what_is_queued_after_it_began, red with the old loop). Not measured: an fsync's latency during a bulk delete. Verified: macOS scripts/test.sh (workspace 2,083/2,083, Shuttle, scripts); GitHub CI on the branch (Linux workspace, guest-sim 3,000 seeds); GCE r539 at 8da75b3cc with Firecracker a2db2b3b0 (fxfs-guest-dax, checks-dax, -delay, -small, script-full-dax, xfstests-fxfs-guest-dax, checks, crash-dax, pgbench-dax all pass).
<!-- SECTION:FINAL_SUMMARY:END -->
