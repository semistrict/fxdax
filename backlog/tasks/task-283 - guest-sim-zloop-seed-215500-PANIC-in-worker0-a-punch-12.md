---
id: TASK-283
title: 'guest/sim zloop seed 215500: PANIC in worker0: a punch: -12'
status: To Do
assignee: []
created_date: '2026-11-09 07:26'
labels:
  - bug
  - sim
dependencies: []
priority: high
ordinal: 705000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Scheduled Hunt run 42554112366 (2026-11-09, at b6ba50fb3), zloop shard 2: seeds 213001-218000 of interleave, 1 failed, not known: 'interleave 215500 200: sim: PANIC in worker0: a punch: -12'. -12 is ENOMEM: a hole punch in the sim's real dax.c failed out of memory. The run's artifact keeps the logs and replay says how to run it again (guest/sim/kernel-tree.sh KERNEL, then replay with KERNEL and BUILD set).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The seed replays and the cause is found
- [ ] #2 Fixed, with the seed added to the sim's known regressions or a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
