---
id: TASK-25.4
title: 'DAX: batch dirty transitions of written pages'
status: To Do
assignee: []
created_date: '2026-10-10 19:08'
labels: []
dependencies: []
references:
  - docs/measurements/gce-fs-bench-2026-10-10T1823Z.md
parent_task_id: TASK-25
type: feature
ordinal: 29000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sequential writes through a DAX window run at 35 MiB/s, against 58 without DAX and 107 on ext4, and mmap writes at a quarter of ext4's rate (docs/measurements/gce-fs-bench-2026-10-10T1823Z.md): each first store to a clean page faults, and the kernel asks the pager to dirty it, a page at a time. Dirty runs of pages per fault, as the pager allows, and make stores to newly written pages not fault again.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Sequential DAX writes at least as fast as without DAX, and mmap writes within 2x of ext4, measured on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
