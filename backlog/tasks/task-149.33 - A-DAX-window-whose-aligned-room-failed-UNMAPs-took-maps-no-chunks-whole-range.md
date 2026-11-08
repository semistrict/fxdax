---
id: TASK-149.33
title: A DAX window whose aligned room failed UNMAPs took maps no chunk's whole range
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-06 03:35'
updated_date: '2026-11-08 04:33'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: low
ordinal: 644000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by guest/sim's sweep (TASK-149.15): a chunk's whole range takes a 2 MiB-aligned slot of the window (fxfs_dax_take_window, gen_pool_first_fit_order_align), and the window's pages an UNMAP failed for are kept from use for good (TASK-44.22). In read_pressure's 8 MiB window one failed UNMAP of a 1 MiB range at 0x16c000 left no aligned slot in the ranges' 4 MiB: every read or write of a large file's chunk fails ENOSPC from then on (before TASK-149.15's fix of the worker's rounds, it waited for ever), most of the window free. A real window of 512 MiB has hundreds of slots, so it takes many failed UNMAPs; but nothing gives the room back. Either a whole chunk's range falls back to unaligned room (PTEs only, no PMD), or the guest retries a failed UNMAP later (at reclaim, or the next sync) and returns the room once one succeeds. guest/sim's unmaps_failed_take_the_window is the scenario to extend.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A chunk's range is had while the window has room for it, aligned or not, or a failed UNMAP's room comes back once an UNMAP of it succeeds
- [ ] #2 unmaps_failed_take_the_window, or a scenario beside it, shows the write succeeding with room left unaligned
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Scenario beside unmaps_failed_take_the_window: the ranges' half in MiB ranges, the first and last MiB of every 4 MiB kept by failed UNMAPs, a 2 MiB write: red on main (ENOSPC).
2. fxfs_dax_alloc_window: with nothing to reclaim, a whole chunk's range takes unaligned room (PTEs; fs/dax.c falls back from a PMD on a misaligned pfn); window_unaligned counts it; dax_huge_maps counts aligned iomaps only.
3. Mutant task-149.33-chunks-only-aligned; check.sh, sweep of both window scenarios, mutants.sh --check.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-07: land-h11 0fb3a96be, CI 42446163807. unmaps_failed_leave_room_unaligned red on main (-28), green with the fix; mutant task-149.33-chunks-only-aligned killed by it, task-149.15-round-releases still killed; check.sh 0 failed of 46 runs and 200 seeds; sweep.sh of both window scenarios 1,830 calls, 0 failed; mutants.sh --check 43/43; check-exclusions 0. The fallback comes only after reclaim found nothing, so a window with reclaimable ranges still gets aligned room (PMDs) as before.
<!-- SECTION:NOTES:END -->
