---
id: TASK-92.42
title: A flush syncs a commit's files one after another
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-23 03:10'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 326000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Measured in TASK-79.2's burst bench (round 1): a durable commit sealing 640 extents in files of their own took about 2.5 s, most of it in Syncs::sync, which fsyncs a commit's files one after another before the directories.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A flush syncs a commit's files in parallel, the directories after them, keeping the order durability needs
- [x] #2 store-bench's burst case shows the durable commit's time before and after
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A flush now syncs a commit's files up to sixteen at once (AT_ONCE), on scoped threads that include the flush's own. It syncs the directories only after every file's sync has returned, so a file is durable before the directory naming it, and the index records still follow both. A failed sync stops the queue and fails the flush without syncing a directory. A host that refuses threads leaves the syncs to fewer threads instead of panicking (f8b41081, ef4b8c57, 7d5061bb, 0ce4a28c). On macOS, where each sync is an F_FULLFSYNC, store-bench burst's durable commit of 640 files went from 2294-2743 ms to 903-991 ms in interleaved runs. The new `store-bench durable N` case (09e8742f) shows no cost for one or two files, and fifteen files' mean flush went from 91.6-107.9 ms to 40.8-52.9 ms. The Shuttle tests for the order, for failure and for refused threads deadlock or fail on the old code and on off-by-one mutants, and pass on macOS and GCE Linux, as does the workspace. The sixteen-at-once bound has not been measured on Linux yet.
<!-- SECTION:FINAL_SUMMARY:END -->
