---
id: TASK-28.8
title: 'Pager: page-ins that do not block threads on reads'
status: To Do
assignee: []
created_date: '2026-10-17 01:32'
labels: []
milestone: m-5
dependencies: []
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
priority: high
type: enhancement
ordinal: 88000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Page-ins call the synchronous ExtentSource::read on two pager threads (crates/fxdax/src/extents.rs PAGER_THREADS, fxfs-platform fxextent.rs). With object storage behind the store, a fetch takes tens of milliseconds, and two blocked threads would serialise every fault on the host.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 ExtentSource reads are asynchronous, and a page-in waiting for a read holds no thread
- [ ] #2 Many page-ins wait on slow reads at once, shown by a test with a source that delays reads
- [ ] #3 The guest suites pass and the GCE bench shows no regression
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
