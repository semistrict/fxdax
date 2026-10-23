---
id: TASK-92.44
title: >-
  Importing many small files costs more per file as the layer grows, inside
  Fxfs/Zircon
status: To Do
assignee: []
created_date: '2026-10-23 03:10'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: low
type: enhancement
ordinal: 328000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Measured with import-bench in TASK-92.37 (round 1): the small-files layer, which removes nothing, grows from about 82 to 137 us an entry between 20,000 and 100,000 files (Fxfs/Zircon with 1 GiB of kernel memory), so the cost is outside the importer.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 import-bench's per-entry cost for small files stays flat from 20,000 to 100,000 files, or the cause is found and filed
- [ ] #2 The bench numbers before and after are in the task
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
