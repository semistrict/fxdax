---
id: TASK-149.22
title: >-
  guest/sim: MAPs that take seeded time, so that seeds reach two MAPs of one
  region at once
status: To Do
assignee: []
created_date: '2026-11-03 07:05'
updated_date: '2026-11-05 03:05'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 569000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
guest/sim's device (kernel/device.c) answers a MAP at once, a scheduling point before and after it. A real host's MAP takes most of a millisecond and others run meanwhile. Holding a seeded share of MAPs for seeded time in interleave and failures would reach races between two MAPs of one region, and others like them, without a scenario per race. It moves every seed and may surface new failures.

What motivated it changed with writable DAX's deletion (37c331a6c). 3,000 seeds missed TASK-44.19's map_to_unlocked, now fxfs_dax_region_map without region->lock (guest/linux/fs/fxfs/dax.c:1394-1436), which needed two writers through slices of one staging extent reaching at once: tests/scenarios' reaches_at_once played it out and went with writable DAX. Copying on write, only reads take slices, and each range MAPs every page it uses: what is left of that race is a region's put UNMAPping less than the host maps, which no oracle in guest/sim checks (guest/sim/README.md:187). sim_device_hold_map (guest/sim/kernel/device.c:434) is still there for scenarios to hold a MAP by hand.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 interleave and failures hold a seeded share of MAPs for a seeded time, from the seed, replaying exactly
- [ ] #2 zloop.sh's 3,000 seeds pass, or what fails is filed
- [ ] #3 guest/sim checks the host's mappings against what each region says it maps, and mutants.sh then finds task-44.19-map-to-unlocked by seeds alone within 3,000, or the notes say why not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
