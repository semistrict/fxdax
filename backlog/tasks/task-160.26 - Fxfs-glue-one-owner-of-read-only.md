---
id: TASK-160.26
title: 'Fxfs glue: one owner of read-only'
status: To Do
assignee: []
created_date: '2026-11-03 13:09'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: low
type: task
ordinal: 597000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Read-only lives in FxFilesystem's options, the guest's Filesystem.read_only, fxfs-posix's read_only, ExtentDevice.writes_nothing and DEVICE_READ_ONLY; flush_volume checks the filesystem's, not the volume's. Explains TASK-42.6.1. Proposal: derive them from one at open.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One source of read-only
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
