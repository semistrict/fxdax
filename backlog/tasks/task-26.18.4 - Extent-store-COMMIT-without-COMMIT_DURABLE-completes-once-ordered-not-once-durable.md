---
id: TASK-26.18.4
title: >-
  Extent store: COMMIT without COMMIT_DURABLE completes once ordered, not once
  durable
status: Done
assignee:
  - '@claude'
created_date: '2026-10-15 08:19'
updated_date: '2026-10-15 08:56'
labels: []
dependencies: []
parent_task_id: TASK-26.18
priority: medium
type: enhancement
ordinal: 58000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-4 lets a plain COMMIT complete once its order is fixed (doc-2 requirement 2: fsync keeps ordering, a crash may lose a suffix); only COMMIT_DURABLE must wait. The host makes every barrier durable (fdatasync of segments and index), so WRITE requests with WRITE_BARRIER take 5-20 ms and every guest fsync pays a host fsync (create-fsync, fsync-append). Recovery then needs data checksums or another way to find a torn epoch.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A plain COMMIT and WRITE_BARRIER complete without waiting for fdatasync
- [x] #2 COMMIT_DURABLE still waits for durability
- [x] #3 A crash test shows the store recovers to a prefix of committed epochs
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_store: commit_ordered (seal, order, reserve the index record's place, queue durability) and flush (group the queued commits: sync data and dirs, write and sync records, remove files).
2. fxdax ExtentStore/daemon/client: COMMIT_DURABLE flag; daemon flushes every 50 ms and before a store goes.
3. libkrun worker honours COMMIT_DURABLE; guest sends it for sync/syncfs/unmount/root commits only.
4. Crash test mixes ordered and durable commits and flushes.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Commits 7d1de76, c0b9295, 81cecd7; libkrun cf11b07. Evidence: extent_store crash test (300 rounds, ordered+durable commits, flushes) passes; workspace nextest macOS 1865/1865 and Linux (GCE) 1871/1871; scripts/upstream-changes.py --check passes; GCE REQUESTS: commit 0.57 ms (was 3.2-3.5 ms durable); create-fsync 285 -> 600-700 ops/s (ext4 508). Guest suites pass.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Plain COMMIT and WRITE_BARRIER now complete once ordered; the daemon makes commits durable within 50 ms in groups, and COMMIT_DURABLE (sync, syncfs, unmount, root) still waits. Verified by the extent_store crash test, workspace tests on macOS and Linux, and GCE guest suites and benchmarks.
<!-- SECTION:FINAL_SUMMARY:END -->
