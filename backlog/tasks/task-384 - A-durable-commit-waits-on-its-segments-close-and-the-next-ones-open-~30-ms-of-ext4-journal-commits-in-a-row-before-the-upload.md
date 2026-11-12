---
id: TASK-384
title: >-
  A durable commit waits on its segment's close and the next one's open, ~30 ms
  of ext4 journal commits in a row before the upload
status: To Do
assignee: []
created_date: '2026-11-12 13:42'
labels:
  - performance
  - extent_store
dependencies:
  - TASK-369
priority: high
ordinal: 763000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-369's clean daemon comparison (crates/fxdax/src/extentd/tests.rs durable_commits_through_the_daemon, ubu1, 25 ms bucket latency, 5 interleaved runs a side, own target dirs): median sync round main 88.5 ms / fix 85.3 ms with the file bucket on the ext4 scratch disk; 71.9 / 68.8 ms with the bucket on /dev/shm, against 37 ms on ZFS home. With the bucket off the disk, the ~30 ms left per round is the store's own syncs: a durable commit uploads its segment, so every round closes one segment and opens the next, and the commit waits, one after another, on ext4 journal commits for the new segment's directory entries, the new framed file, and the framed files' directory, before the upload goes. TASK-369's fixes (duplicate entry syncs removed: 1,578 -> 863 ms of entry-sync time over 40 rounds) don't touch this chain. Change what an upload must make durable locally first: the bucket object is what makes the commit durable, so the local framed file and directory entries may not need to be durable before it (recovery re-derives them from the WAL/raw file?), or the next segment can be opened ahead of time off the commit's path. Decide it against doc-12's recovery rules and the TLA+ model (specs/bucket), with crash tests. TASK-371's AC #4 (25 durable syncs/s) and doc-12's 50 ms median (TASK-322) depend on it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 durable_commits_through_the_daemon on ubu1's ext4 scratch disk with the bucket on /dev/shm: median round within 5 ms of the bucket's round trip
- [ ] #2 Crash tests and the sim show no commit acknowledged durable is lost; the recovery rule that makes it safe is written in doc-12 (and the TLA+ model if it covers it)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
