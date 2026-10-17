---
id: TASK-28.7
title: Leases and takeover; opening a branch on a host without it
status: To Do
assignee: []
created_date: '2026-10-17 01:32'
labels: []
milestone: m-5
dependencies:
  - TASK-28.5
  - TASK-28.6
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
type: feature
ordinal: 87000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-7 Fencing and decision 4: takeover is explicit, a conditional update of the lease then a fence segment; the old writer finds out at its next upload and answers EROFS (doc-4). This is also what lets milestone 4 (m-6) resume a VM elsewhere.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A host takes a branch lease and fences it; the previous writer next upload fails, it answers EROFS, and nothing it uploads afterwards is read, shown by a test
- [ ] #2 A branch opens on a host with an empty local store and serves reads from object storage
- [ ] #3 Extents minted after a takeover never reuse an ID any earlier epoch uploaded
- [ ] #4 A fork child record is created once the parent has uploaded through the fork point
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
