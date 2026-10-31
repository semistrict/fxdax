---
id: TASK-146.2
title: write(2) on DAX without a window round trip in the common case
status: To Do
assignee: []
created_date: '2026-10-31 22:15'
labels: []
dependencies: []
parent_task_id: TASK-146
priority: high
ordinal: 498000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Their ask 2. 122,027 of 134,078 maps per run are evacuation's, not the write's own: find what write(2) itself waits on first (its 605 us includes waits behind evacuation). Serving write(2) through the device WRITE path isn't their 72 us: that is the page cache, which a DAX inode hasn't, and a device WRITE is a 340+ us round trip. See TASK-131.2, TASK-109.1.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 write_iter's time split: own MAPs, waits behind evacuations, copying
- [ ] #2 A chosen design moves pgbench-dax past no DAX's synchronous_commit=off tps, or says why it can't
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
