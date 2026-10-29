---
id: TASK-119.10
title: >-
  Bookkeeping assertions that fail tests: nr_moved vs moved[], region indices
  valid, placed pages at the extent Fxfs says, no MAP pending into freed window
  space; DAX diagnostics WARN
status: In Progress
assignee: []
created_date: '2026-10-28 19:45'
updated_date: '2026-10-29 09:23'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: high
ordinal: 448000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Fable/Astra: at place, ask Fxfs (fxfs_dax_extents) where each page is and WARN unless it is the run mapped; assert nr_moved and moved[] agree at relocate/prune/resize_end; a bounded event history per page (extent, seal count, MAP id) printed on failure. The runner fails tests on WARNING:, not pr_warn.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Assertions behind a debug param, on in checks-dax-delay
- [ ] #2 A hand mutant breaking each is caught by its assertion
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-29, branch t3/task-112-6: fxfs_dax_moved_index/moved_region WARN when a moved page's region index is none of its range's (every decode of moved[], prune's __set_bit included, which wrote past its bitmap otherwise); fxfs.dax_check_places: as an evacuation places a run, asks Fxfs (fxfs_dax_extents, outside the files lock) where its pages are and WARNs unless at the extent and offset its MAP mapped from; on in checks-dax-delay. Mutants evacuation_maps_elsewhere and prune_misnumbers in scripts/guest-mutants-gce.py. nr_moved is not on main (TASK-112.5's wip), so nothing to check of it; the per-page history is fxfs.dax_trace (TASK-124). Pending: GCE r214 (quota: after a running host frees).

GCE r214 (kernel built per mutant): evacuation_maps_elsewhere and prune_misnumbers each failed checks-dax-delay in round 1 on fxfs_dax_check_placed's WARN (the run mapped from one extent offset, Fxfs holding it at another); prune_misnumbers lands on another valid region index, so check_placed, not the index WARN, caught it: the index WARN's own kill is unshown. evacuation_unlocked also tripped check_placed, but with -E2BIG: the check asked Fxfs for one piece; 4f1b76226 reads them all (a false WARN on correct code otherwise). The script called all three BROKEN, reading remote.log for the failed round that e2e.log says: fixed there too.

GCE r221: checks-dax-delay with fxfs.dax_check_places 2 rounds passed after check_placed reads every piece and looks below the file's size (a truncate's pages past it, which resize_end unmaps after, were false WARNs). evacuation_maps_elsewhere and prune_misnumbers killed by it (r214); the region index WARN's own kill is still unshown.
<!-- SECTION:NOTES:END -->
