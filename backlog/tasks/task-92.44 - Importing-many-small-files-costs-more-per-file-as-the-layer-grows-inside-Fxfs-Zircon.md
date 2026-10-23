---
id: TASK-92.44
title: >-
  Importing many small files costs more per file as the layer grows, inside
  Fxfs/Zircon
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 03:10'
updated_date: '2026-10-23 06:12'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Round 2a: not the cause of TASK-79.70; memory stays Normal throughout. import-bench, small-files layer, best of 2 on a loaded Mac: 20,000 files 63.0 us/entry before TASK-79.70 and 63.4 after; 100,000 files 71.5 and 70.9 (round 1's 82 -> 137 us was not reproduced; the growth now is about 13%). Instructions per entry rise from 1.84 M to 2.48 M, so it is work, not latency. Cause: Fxfs's allocator merges all its layers on every journal flush, quadratic over the extent store's one record per allocation (74 compactions / 16.4 MB / 98 ms at 20k files, 395 / 401 MB / 2,593 ms at 100k), filed as TASK-92.46; whether imports should run Fxfs on two threads is TASK-92.47.
<!-- SECTION:NOTES:END -->
