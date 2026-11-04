---
id: TASK-164.2
title: 'Guest: DAX writes copy on write, allocated at writeback'
status: To Do
assignee: []
created_date: '2026-11-04 07:58'
labels: []
dependencies: []
parent_task_id: TASK-164
priority: high
type: task
ordinal: 612000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Behind fxfs.dax_cow (doc-13 stage 2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A dirty set with reservations; writeback by fxfs_write_many from the window, committing after the bytes
- [ ] #2 Rebinding to the new sealed extent under the semaphore held exclusively; the budget throttles under private_max; holes as zeros; truncate, punch, clone and freeze
- [ ] #3 The DAX and crash suites pass with it on
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
