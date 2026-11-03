---
id: TASK-149.22
title: >-
  guest/sim: MAPs that take seeded time, so that seeds reach two MAPs of one
  region at once
status: To Do
assignee: []
created_date: '2026-11-03 07:05'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 569000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
guest/sim's device (kernel/device.c) answers a MAP at once, a scheduling point before and after it. 3,000 seeds of interleave and failures missed TASK-44.19's map_to_unlocked (fxfs_dax_map_to without region->lock), which needs a MAP in flight while another of the same region, further, ends, then a seal before the grow worker maps on: tests/scenarios' reaches_at_once plays it out with sim_device_hold_map. A real host's MAP takes most of a millisecond and others run meanwhile. Holding a seeded share of MAPs for seeded time in interleave and failures would reach this class, and others like it, without a scenario per race. It moves every seed and may surface new failures.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 interleave and failures hold a seeded share of MAPs for a seeded time, from the seed, replaying exactly
- [ ] #2 mutants.sh finds task-44.19-map-to-unlocked by seeds alone (without reaches_at_once) within 3,000, or the notes say why not
- [ ] #3 zloop.sh's 3,000 seeds pass, or what fails is filed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
