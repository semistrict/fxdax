---
id: TASK-146.1
title: >-
  A timeline of one device round trip: where 300+ us goes between the guest's
  kick and its wakeup, and whether polling saves it
status: To Do
assignee: []
created_date: '2026-10-31 22:15'
updated_date: '2026-10-31 23:37'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31: virtio_extent.spin_ns=1000000 against the default, pgbench-dax, 5 runs, two hosts of one type: r358 1244/1133/1109/1109/1125 (mean 1144) against r362 1160/1063/1003/985/1031 (mean 1049): +9%. MAP request 235 us against 358; polled 973018 against 17 of about 1.1M. Guest CPU: system 41810 against 30973 ticks, idle 21759 against 31253. Not a default yet: it needs one host's A/B (AGENTS.md), and a VM spinning takes host CPU others may want: density is what the sandbox team bought DAX for.
<!-- SECTION:NOTES:END -->
