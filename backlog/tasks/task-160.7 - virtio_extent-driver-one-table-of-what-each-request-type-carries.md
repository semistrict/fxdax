---
id: TASK-160.7
title: 'virtio_extent driver: one table of what each request type carries'
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: low
type: task
ordinal: 578000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Each request type's wire contract is spread over four switches in driver.c (vext_writes, vext_args, vext_data_len, vext_root_len); 7afb19e67 had to touch several. Proposal: one row per type.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One table describes every request type
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
