---
id: TASK-92.7
title: >-
  Each collector pass GETs every branch record ever made and recovers every
  fork, one at a time
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-22 21:06'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 282000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
records() GETs each record in turn, including deleted forks' records, which never change and are never removed (collector.rs:86-94). live() recovers each branch in turn (113-136), and a fork without its own checkpoint downloads its parent's again (reader.rs:228-235). The pass holds every state (141-147), visits each inherited extent once per child (166-186), and deletes one awaited key at a time (69-72). 300 forks of a 100k-extent base mean about 30M entries in memory and tens of seconds of CPU per pass. 50k forks ever made mean about 25 minutes of serial GETs per pass.

Where: crates/extent_remote/src/collector.rs:69-72, crates/extent_remote/src/collector.rs:86-94, crates/extent_remote/src/collector.rs:113-136, crates/extent_remote/src/collector.rs:141-147, crates/extent_remote/src/collector.rs:166-186, crates/extent_remote/src/reader.rs:228-235.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there. records() GETs each record serially, deleted ones included, and remembers nothing across passes (crates/extent_remote/src/collector.rs:84-101). live() recovers each branch serially, and childless forks re-read their parent's state (103-140; recovery.rs state_at). Pass::new walks every live state's extents, inherited ones included, once per branch (157-190), holding all states. Deletes are one awaited request at a time (70-73). Related: TASK-28.10, TASK-79.3, TASK-79.18, TASK-76.
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
A Collector now remembers the branch records it read, with their versions, and reads them 32 at once, re-reading only changed ones. It recovers 16 branches at once through a shared checkpoint reader, takes what a round's forks inherited once as shared layers, and deletes 32 keys at once. In passes_read_records_and_checkpoints_once_and_delete_at_once, a pass went from 9 serial record reads, 7 checkpoint reads and 8 serial deletes to one round each, and a second pass reads no record. The 100-fork benchmark went from 23.6 s and 1.6 GiB to 0.60 s and 132 MiB. The test passes on macOS and on Linux (GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
