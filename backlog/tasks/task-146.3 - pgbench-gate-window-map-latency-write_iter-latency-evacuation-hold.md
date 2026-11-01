---
id: TASK-146.3
title: 'pgbench gate: window map latency, write_iter latency, evacuation hold'
status: Done
assignee: []
created_date: '2026-10-31 22:15'
updated_date: '2026-11-01 01:59'
labels: []
dependencies: []
parent_task_id: TASK-146
priority: medium
ordinal: 499000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Their ask 4. /proc/fs/fxfs has window_maps_ns, window_map_requests and dax_evacuate_hold_ns already; write_iter latency it hasn't.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 scripts/dax-gate.sh reports all three and fails on a regression
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-01: scripts/pgbench-probes.py, run by dax-gate.sh on pgbench-dax's console: a window MAP request, a write(2) (dax_writes_ns, new in c5ab5a789) and an evacuation's hold, each failing the gate at half as much again as r365 measured: 560, 560 and 1800 us. Tests: test_pgbench_probes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The gate reports the three probes the sandbox team asked for and fails past their ceilings, from the guest's own counters, so their kprobes aren't needed.
<!-- SECTION:FINAL_SUMMARY:END -->
