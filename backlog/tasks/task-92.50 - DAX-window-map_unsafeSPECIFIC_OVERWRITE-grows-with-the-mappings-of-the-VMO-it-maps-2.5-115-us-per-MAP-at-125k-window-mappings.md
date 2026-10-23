---
id: TASK-92.50
title: >-
  DAX window map_unsafe(SPECIFIC_OVERWRITE) grows with the mappings of the VMO
  it maps: 2.5 -> 115 us per MAP at 125k window mappings
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 18:12'
updated_date: '2026-10-23 19:20'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: high
type: enhancement
ordinal: 356000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by a probe of extentd's MAP under pgbench with DAX (TASK-79.90's investigation; GCE, five 60 s runs; probe log in the review session's scratchpad r5/mapprobe1, the probe saved as r5/probe-map.patch and probe-map.rs). MAP time per MAP grows from about 29 us to about 180 us as the guest's DAX window fills, and it predates the review's fixes (runs at 07ba6634 and 66d56a43 grow 116 -> 292 us). All of the growth is in one call: the window VMAR's map_unsafe with SPECIFIC_OVERWRITE in Window::map (crates/fxdax/src/dax.rs). It costs 2.5 us per MAP at first, 4 us at 20k window mappings, 27 at 30k, 70-130 at 37k-90k, and about 105-117 us once the window's mappings level off near 125k. Lock waits, lookups, the commit, MAP_RANGE's populate, the stage call and the sends to the VM process stay flat. The same call from seals' and frees' remaps costs 9-25 us at the same mapping count; MAPs of the open extent map the one open extent's VMO each time, so the likely cause, not yet shown, is an O(n) step in the zircon port's create-mapping or overwrite path that walks that VMO's mappings (crates/zircon/src/vm/vm_address_region.rs, vm_mapping.rs, vm_object.rs, vm_cow_pages.rs). Upstream Zircon's behaviour there decides whether the fix is fxdax's.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A zircon test mapping one VMO tens of thousands of times into one VMAR with SPECIFIC_OVERWRITE shows the cost per mapping before, and that it no longer grows with the VMO's mappings after
- [ ] #2 fxfs-guest-pgbench-dax on GCE shows extentd's MAP time flat across its runs, before and after
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
r6/zircon-map: the kernel's map and overwrite path is log-time throughout (the VMAR's subregions, the VMO's mappings and the ptes are trees keyed by address), as upstream's. The premise that one open-extent VMO is mapped ~100k times does not hold: each open extent gets a VMO of its own, and commit_and_seal remaps its window mappings onto the sealed extent at every commit; what grows is the window's mapping count. A real deviation was found on the way and is being fixed on that branch: the port kept a VMO's mappings by pointer, so a range change (zero-fork commit, page-in, supply, eviction, writeback) visited every mapping of the VMO, where upstream keeps them by offset in an augmented tree and visits the range's alone (371 us -> 1.65 us per range change at 100k mappings). The MAP growth itself fits a waited STAGE_UNMAP per overwrite of a populated window page (map_unsafe us = -194 + 99 x sends per MAP, r = 0.96; sends per MAP rise from 2.06 to 3.0-3.3 when the guest's window fills), a round trip to the VM process inside map_unsafe under the aspace, VMO and MMU locks; seals' and frees' remaps defer that wait. Not measured yet: Acks::wait's time per Window::map (probe in the review session's scratchpad, r6/probe-map-wait.patch). AC #1 as worded (the kernel's cost growing) is not what was found.
<!-- SECTION:NOTES:END -->
