---
id: TASK-149.7
title: Device errors in the model test
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 06:13'
updated_date: '2026-11-03 02:21'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 508000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Failed reads, writes and flushes are in no model test. The rule to check: a clean error or read-only, never a panic, never a lost fsync, and nothing durable changed. set_op_callback exists on FakeDevice.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Sequences inject failures of reads, writes and flushes, seeded
- [x] #2 A failed fsync is reported, and what was synced before it stays durable
- [x] #3 A filesystem that hit an error remounts and passes fsck
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Seeded failures of reads, writes, flushes and frees through set_op_callback, one kind or all, in the model test; a flush that fails seals nothing.
2. A sequence ends at the first operation a failure defeats; what the device then holds, crashed and as it stands, must remount, pass fsck and hold what was durable, or what the operation got as far as.
3. Panics in any task fail the test, through a hook. A directed test of a failed fsync.
4. Controls: swallow a flush's error, and panic in a background task; both must fail.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Failures of reads, writes, flushes and frees, one kind or all, seeded. Controls: a swallowed flush error fails on [Create, Crash, Write, Fsync], a panic in a background task fails naming it. 11,900 sequences found nothing in Fxfs. Not modelled: a write that fails yet lands, or a device that stays failed.

2026-11-02: workspace at main passes on macOS (2,010 of 2,010, 1fc28f48c) and Linux (GCE r530, 2,169 of 2,169, Shuttle suites too).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Seeded device failures in the model test, with both rules shown by controls; no bug found.
<!-- SECTION:FINAL_SUMMARY:END -->
