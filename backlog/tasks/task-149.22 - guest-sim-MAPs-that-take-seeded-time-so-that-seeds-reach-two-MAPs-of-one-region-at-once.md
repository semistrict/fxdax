---
id: TASK-149.22
title: >-
  guest/sim: MAPs that take seeded time, so that seeds reach two MAPs of one
  region at once
status: Done
assignee: []
created_date: '2026-11-03 07:05'
updated_date: '2026-11-08 04:46'
labels:
  - wontfix
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 569000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
guest/sim's device (kernel/device.c) answers a MAP at once, a scheduling point before and after it. A real host's MAP takes most of a millisecond and others run meanwhile. Holding a seeded share of MAPs for seeded time in interleave and failures would reach races between two MAPs of one region, and others like them, without a scenario per race. It moves every seed and may surface new failures.

What motivated it changed with writable DAX's deletion (37c331a6c). 3,000 seeds missed TASK-44.19's map_to_unlocked, now fxfs_dax_region_map without region->lock (guest/linux/fs/fxfs/dax.c:1373-1416, at dde8d58d3), which needed two writers through slices of one staging extent reaching at once: tests/scenarios' reaches_at_once played it out and went with writable DAX. Copying on write, only reads take slices, and each range MAPs every page it uses: what is left of that race is a region's put UNMAPping less than the host maps, which no oracle in guest/sim checks (guest/sim/README.md:196; guest/mutants/task-44.19-map-to-unlocked.patch, Killer: none, since b7653f781 made the simulator's and the guest's mutants one set). sim_device_hold_map (guest/sim/kernel/device.c:434) is still there for scenarios to hold a MAP by hand.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 interleave and failures hold a seeded share of MAPs for a seeded time, from the seed, replaying exactly
- [ ] #2 zloop.sh's 3,000 seeds pass, or what fails is filed
- [ ] #3 guest/sim checks the host's mappings against what each region says it maps, and guest/sim/mutants.sh then finds guest/mutants/task-44.19-map-to-unlocked.patch by seeds alone within 3,000, its header's Killer naming what does, or the notes say why not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05 triage: citations brought to dde8d58d3; the mutants moved from guest/sim/mutants to guest/mutants (b7653f781).

2026-11-08: closed as won't fix, the user's decision (OPEN_QUESTIONS.md, b0d7d00d2): what motivated it, races of two MAPs of one region under writable DAX, went with its deletion (37c331a6c); what is left, a region's put UNMAPping less than the host maps, isn't worth a seeded-time device.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Won't fix, the user's decision of 2026-11-08 (OPEN_QUESTIONS.md, b0d7d00d2): its motivating race went with writable DAX (37c331a6c); nothing built.
<!-- SECTION:FINAL_SUMMARY:END -->
