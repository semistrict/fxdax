---
id: TASK-28.1
title: >-
  Simulation of branches in object storage: fencing, uploads, forks and garbage
  collection
status: To Do
assignee: []
created_date: '2026-10-17 01:31'
labels: []
milestone: m-5
dependencies: []
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
priority: high
type: task
ordinal: 81000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-2 warns that garbage collection over object storage with O(1) forks is subtle and needs a model and tests before code. doc-7 gives the protocol (create-only dense segments as commit and fence, leases, fork records, moves, a collector that alone deletes); the survey found real bugs of this shape in SlateDB (RFC 0026, 0029) and rocksdb-cloud. The simulation is the executable form of doc-7 that the later tasks implement against.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A deterministic simulation (seeded) runs writers, forks, takeovers, branch deletions and the collector over an object store that injects errors after writes land, delays, reordering and crashes at every step
- [ ] #2 It checks after every step that no extent a live branch holds is unreadable, and at quiescence that every object holding no live extent is deleted, against a mark-and-sweep oracle
- [ ] #3 It covers the ten cases doc-7 lists under Testing, each with a named test that fails if the rule it depends on is removed
- [ ] #4 Thousands of seeds run in the workspace tests within the 5-minute budget
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
