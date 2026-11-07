---
id: TASK-149.20.4
title: No oracle checks a DAX region's put unmapping less than the host maps
status: Done
assignee:
  - '@claude'
created_date: '2026-11-07 13:37'
updated_date: '2026-11-07 20:42'
labels:
  - testing
dependencies: []
parent_task_id: TASK-149.20
priority: medium
type: task
ordinal: 669000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-149.20 reading guest/mutants/task-44.19-map-to-unlocked.patch (Killer: none): copying on write, each range MAPs every page it uses, and what is left of the mutant is a region's put UNMAPping less than the host maps, which no oracle in guest/sim or the guest suites checks. A leak of host mappings (vm.max_map_count, the daemon's window state) would pass every test.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 An oracle (guest/sim's, or the daemon's window state at a check's end) compares what the guest unmapped with what the host maps
- [x] #2 task-44.19-map-to-unlocked gets a killer, or its header says why none can
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. guest/sim: sim_device_check_unmapped(when), the unmount's check made callable mid-run: nothing mapped once nothing of the guest's holds window space.
2. Scenario region_maps_at_once: a file and a clone of its first pages, slices of one region; the clone's reader's whole-region MAP fails, its retry is held; the file's reader maps the whole region; drops, then the check.
3. task-44.19-map-to-unlocked's killer: that scenario; mutants.sh kills it.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done (commit 54141f0e0). The oracle already existed at the unmount (sim_device_check_unmounted); the mutant escaped it because the end re-reads the files, whose region takes the same window space and UNMAPs it whole. sim_device_check_unmapped(when) makes it callable mid-run. region_maps_at_once: a file of 8 pages and a clone of its first 4 are slices of one region. The clone's reader's whole-region MAP fails (sim_device_fail) and its 4-page MAP is held (sim_device_hold); the file's reader MAPs the whole region meanwhile. With the ranges dropped, the check finds nothing mapped. The scenario asserts its own premises: the held MAP is 4 pages, and there is one region. task-44.19-map-to-unlocked's killer is now this scenario: mutants.sh kills it ('window page 2004 maps an extent'). zloop -s 100, the scenario's sweep (44 calls) and mutants.sh --check (42) passed; guest-mutants-gce.py --check: 53.

CI 42410439072 on land-r2 4ab1eb0f2: all 15 jobs green, macOS and Linux workspace, guest, guest-sim and its sweep, and clippy scripts among them.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
guest/sim checks mid-run that the host maps nothing once nothing of the guest's holds window space (sim_device_check_unmapped). Scenario region_maps_at_once races two readers' MAPs of one region and kills task-44.19-map-to-unlocked, which had no killer. zloop, sweep and mutants checks pass.
<!-- SECTION:FINAL_SUMMARY:END -->
