---
id: TASK-146.2
title: write(2) on DAX without a window round trip in the common case
status: To Do
assignee: []
created_date: '2026-10-31 22:15'
updated_date: '2026-10-31 23:53'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31, GCE r364, pgbench-dax with every commit asynchronous, tps 2545/2536/2560/2623/2569 (their 2436): 1,409,801 writes, 251 us each: waiting for the inode's lock 113.7 us (45%), mapping under it 44.6 (18%), Fxfs 31.2 (12%), the gate 10.5 (4%), the rest, the copy among it, 51 us. 0.98 Fxfs transactions a write. Evacuation: 994,451 of 1,203,185 maps (83%). A MAP request 674 us; a device read 1089 us, a WRITE 1537 us, against about 500 and 340 uncontended: the VMM's one thread is behind evacuation. So the writes queue behind each other: a MAP about 1 write in 15 holds the exclusive inode lock for 674 us. Next: rarer write-path MAPs (region step), then the inode lock shared for overwrites (ext4 DAX does), then evacuation's traffic.
<!-- SECTION:NOTES:END -->
