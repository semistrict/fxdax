---
id: TASK-79.18
title: >-
  A stale writer can recreate a collected segment, and readers that resolve by
  key read its bytes as their extent
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 206000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A fenced writer's retried create can land at a segment key the collector deleted. Readers drop the footer and cache tails by key, so one whose index predates the compaction reads the stale writer's bytes as its extent, with valid CRCs. The sim's uniform scheduler practically never produces the interleaving that doc-7 says is covered.

Where: crates/extent_remote/src/writer.rs:366, crates/extent_remote/src/bucket.rs:176, crates/extent_remote/src/collector.rs:258, crates/extent_remote/src/reader.rs:282, crates/extent_remote/src/fetch.rs:448, crates/extent_remote/src/sim/world.rs:316, crates/extent_remote/src/sim/mod.rs:211.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-28.1, TASK-28.10, TASK-44.5. No task covers a fenced writer's retried create landing at a key the collector deleted, or readers trusting a key-cached tail. TASK-44.5 (To Do), TLA+ over doc-7, is the exhaustive check that would reach the interleaving the sim misses.
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

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Readers no longer return a stale writer's recreated segment as their extent. A fetcher now reads each extent at the place the object's own footer gives, follows moves when the object does not name the extent, and re-resolves once with a fresh tail on a CRC failure. The first tail read grew to 64 KiB, so a large segment's footer comes in one GET. Verified by two fetch tests that failed before the fix (wrong bytes, and Corrupt) and by the simulation's recreated profile, which fails on the pre-fix code at seed 8. All of them pass on macOS and Linux (GCE). The simulation work itself is TASK-79.73.
<!-- SECTION:FINAL_SUMMARY:END -->
