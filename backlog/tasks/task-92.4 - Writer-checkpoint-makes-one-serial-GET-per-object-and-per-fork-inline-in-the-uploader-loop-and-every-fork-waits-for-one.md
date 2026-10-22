---
id: TASK-92.4
title: >-
  Writer::checkpoint makes one serial GET per object and per fork, inline in the
  uploader loop, and every fork waits for one
status: To Do
assignee: []
created_date: '2026-10-22 21:06'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: high
type: enhancement
ordinal: 279000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Each checkpoint builds a fresh MovesMaps and probes moves/<key> once per object holding a live extent (writer.rs:453-460, reader.rs:324-327; usually a 404). It then GETs the record of every live fork, one at a time (466-471). The uploader awaits it inline every 64 segments, polling no uploads or waiters meanwhile (remote.rs:651-656), and before answering any fork (687-689). Reopening a base re-creates every child's record, at 2 requests each (writer.rs:188, 423-439). With 300 forks of a base, each new fork waits for at least 300 record GETs (6-9 s) plus one GET per object, O(F^2) over a storm, and long runs stall in wait_lag (extents.rs:1275).

Where: crates/extent_remote/src/writer.rs:453-460, crates/extent_remote/src/writer.rs:466-471, crates/extent_remote/src/writer.rs:188, crates/extent_remote/src/writer.rs:423-439, crates/extent_remote/src/reader.rs:324-327, crates/fxdax/src/remote.rs:651-656, crates/fxdax/src/remote.rs:687-689, crates/fxdax/src/extents.rs:1275.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there. Writer::checkpoint builds a fresh MovesMaps and follows every extent serially; each new object key costs one moves/<key> footer GET (crates/extent_remote/src/writer.rs:330-340; moves.rs:35-50). It then GETs each fork's record one at a time (writer.rs:342-348). The uploader awaits it inline every CHECKPOINT_EVERY segments (crates/fxdax/src/remote.rs:658-662) and in acknowledged() before forks are answered (remote.rs:820-827). record_forks' `recorded` set is per Writer and starts empty (writer.rs:147, 300-317). So a reopened writer calls record_fork for every child, and each existing child costs a failed create plus a GET (branch.rs:30-44). Related: TASK-90, TASK-39.3, TASK-91.3, TASK-79.39.
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
