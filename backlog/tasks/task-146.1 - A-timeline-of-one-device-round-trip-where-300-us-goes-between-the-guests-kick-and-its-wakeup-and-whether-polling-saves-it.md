---
id: TASK-146.1
title: >-
  A timeline of one device round trip: where 300+ us goes between the guest's
  kick and its wakeup, and whether polling saves it
status: To Do
assignee: []
created_date: '2026-10-31 22:15'
labels: []
dependencies: []
parent_task_id: TASK-146
priority: high
ordinal: 497000
---

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One MAP and one READ timed at each hop, guest, VMM worker, daemon, stage operations, ack, completion
- [ ] #2 virtio_extent.spin_ns tried at 100, 600 and 1000 us on pgbench-dax, tps and CPU reported
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
