---
id: TASK-160.17
title: 'extent_store: the uploader''s holds with one owner, acknowledged in one call'
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 588000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Segments not yet uploaded are owned by Uploads.unuploaded and mirrored by hand into Segment.uploading; frees kept for the uploader mirrored into three counters; one acknowledgement is release(epoch) then uploaded(n), which must agree. Explains TASK-85, 79.89, 126f21bcf. Proposal: Segments owns the hold (Phase::Closed{held}), and an Uploader handle with acknowledged(through). After TASK-153.8 and 41.1.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One owner of the upload hold
- [ ] #2 One acknowledgement call
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
