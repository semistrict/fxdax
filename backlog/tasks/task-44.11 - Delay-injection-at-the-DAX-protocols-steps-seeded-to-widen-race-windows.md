---
id: TASK-44.11
title: 'Delay injection at the DAX protocol''s steps, seeded, to widen race windows'
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
updated_date: '2026-10-24 23:57'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: medium
ordinal: 390000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Races between a check and its act (TASK-99's mapped check against a MAP in flight) need microsecond windows to overlap and show once in hours. Random delays at protocol points (before a MAP is sent, after it returns, between a gate check and its use, in the sealer) make them overlap routinely. fxfs.follow_apply_delay_us does this for followers alone.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Guest module parameters and host environment settings add seeded random delays at named protocol steps
- [ ] #2 faults_unheld and seal_race fail with the TASK-99 fix reverted and delays on, without fxfs.dax_fail_held
- [x] #3 The guest suites pass with delays on
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
c7dacce4d: fxfs.dax_delay_us / dax_delay_seed, seeded sleeps a quarter of the time at a chunk's map, a slice's, a write's (between Rust's answer and the gate), after a MAP's reply in map_to, before region_open. GCE r81: fxfs-guest-dax, -checks-dax, -follow-dax, -pgbench-dax pass at 300 us. AC2 left: TASK-99's fix reverted (guest-mutants region_open_unlocked) failing faults_writes_compaction/seal_race with delays, without fail_held.
<!-- SECTION:NOTES:END -->
