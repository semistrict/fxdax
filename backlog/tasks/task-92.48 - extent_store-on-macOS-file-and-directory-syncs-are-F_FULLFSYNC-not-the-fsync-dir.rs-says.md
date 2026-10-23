---
id: TASK-92.48
title: >-
  extent_store on macOS: file and directory syncs are F_FULLFSYNC, not the fsync
  dir.rs says
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-23 06:12'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: low
type: task
ordinal: 339000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found reviewing TASK-92.42 (round 2a). crates/extent_store/src/dir.rs:106-107 says a macOS file sync is a plain fsync, leaving the drive cache to barrier's single F_FULLFSYNC, but sync_cached, sync and sync_dir call sync_all, which Rust's std implements as F_FULLFSYNC on Apple (as it does sync_data), so every file and directory sync flushes the drive cache. macOS is a development host, but every macOS store-bench figure (TASK-92.42's included) measures drive-cache flushes. Using libc::fsync there has a durability trade-off to state.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 dir.rs's macOS syncs do what its doc says, or the doc says what they do and why
- [x] #2 store-bench's burst figures on macOS before and after are in the task
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
On macOS, a HostDir file or directory sync is now an F_BARRIERFSYNC, falling back to F_FULLFSYNC and then fsync. This leaves the drive's cache to barrier's F_FULLFSYNC, as HostDir's doc said. The doc now states the trade-off: on macOS a sync is durable against power loss only once a barrier follows, and every point where the store promises durability ends in one. barrier's own fallback is now an fsync instead of std's sync_all (dbe46395, f7b08448, 53936f1e). store-bench burst on macOS, interleaved before and after: the durable commit took 758.6-887.5 ms before and 274.2-331.0 ms after, and its flush 545-669 ms before and 59-121 ms after. Over twenty interleaved rounds, the flush's median went from 620 ms (quartiles 572-664) to 175 ms (168-209), and durable 1's from 9.8 ms to 4.1 ms. Linux is unchanged (fdatasync, and fsync for directories), and the workspace passes on macOS and GCE Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
