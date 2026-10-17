---
id: TASK-28.5
title: 'Uploader: ordered uploads with a bounded lag'
status: To Do
assignee: []
created_date: '2026-10-17 01:32'
labels: []
milestone: m-5
dependencies:
  - TASK-28.1
  - TASK-28.3
  - TASK-28.4
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
priority: high
type: feature
ordinal: 85000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-2 requirement 11: a commit is safe once uploaded, and a host failure loses at most the unuploaded suffix. doc-4 already says COMMIT_DURABLE waits for the upload once object storage exists. doc-7 decision 3: at the lag bound, commits stall rather than the bound break.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Closed segments and extents of their own upload in parallel, extents before the segments naming them, and a segment counts as committed only once every earlier one is
- [ ] #2 COMMIT_DURABLE completes once its commit is uploaded
- [ ] #3 The lag is bounded in bytes and time; at the bound COMMIT stalls while WRITE and reads go on, shown by a test with an unreachable store
- [ ] #4 After a host crash, reopening uploads what was missing and loses nothing durable locally, shown by a crash test
- [ ] #5 Uploads retry with backoff, and a branch falling behind is reported
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
