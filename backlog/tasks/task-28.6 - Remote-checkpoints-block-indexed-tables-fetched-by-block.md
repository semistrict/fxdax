---
id: TASK-28.6
title: 'Remote checkpoints: block-indexed tables fetched by block'
status: To Do
assignee: []
created_date: '2026-10-17 01:32'
labels: []
milestone: m-5
dependencies:
  - TASK-28.5
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
type: feature
ordinal: 86000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The local checkpoint is one record of every extent, which a host resuming a large branch would have to fetch whole. doc-7 Checkpoints: a sorted table in 64 KiB blocks with CRCs, a block index and a trailer, so one extent costs a suffix GET, the index and one block.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Checkpoints are written in the doc-7 format, locally and to object storage, on the doc-7 cadence, and the two newest are kept
- [ ] #2 A host looks an extent up without loading the whole checkpoint, shown by a test counting the bytes fetched
- [ ] #3 A branch state is its newest checkpoint plus the footers after it, shown by a test that compares it with the local index
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
