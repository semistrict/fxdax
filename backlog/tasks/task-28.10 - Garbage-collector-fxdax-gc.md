---
id: TASK-28.10
title: 'Garbage collector: fxdax-gc'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 01:32'
updated_date: '2026-10-17 04:27'
labels: []
milestone: m-5
dependencies:
  - TASK-28.1
  - TASK-28.2
  - TASK-28.6
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
type: feature
ordinal: 90000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-7 Garbage collection: the only thing that deletes. Marks from every live branch head and fork points, compacts mostly-dead segments through moves records, deletes orphans, and finishes branch deletions, coordinating through object storage alone (doc-2 requirement 13).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 It reclaims freed extents and compacts mostly-dead segments, and readers on other hosts with older indexes still read moved extents
- [x] #2 It holds a lease; two collectors, or one crashing or fenced at any step, lose nothing
- [x] #3 It passes the simulation (TASK-28.1) with the real code in place of the model
- [x] #4 A collection that cannot read a branch metadata stops rather than deleting
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
The collector is extent_remote::collector, written and checked in TASK-28.1: the simulation runs it as the real code (compaction through moves read by stale indexes: collectors_stopping_and_racing; lease and two collectors, crashes: same; a pass that cannot read a live branch metadata returns an error and deletes nothing). fxdax-gc (crates/extent_remote/src/bin) runs passes over a bucket URL, once or --every SECONDS, probing conditional writes first; scripts/test-rustfs.sh runs it against RustFS after the store tests.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxdax-gc binary over extent_remote::collector (simulation-checked); runs against RustFS in scripts/test-rustfs.sh.
<!-- SECTION:FINAL_SUMMARY:END -->
