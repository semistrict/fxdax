---
id: TASK-92.50
title: >-
  DAX window map_unsafe(SPECIFIC_OVERWRITE) grows with the mappings of the VMO
  it maps: 2.5 -> 115 us per MAP at 125k window mappings
status: To Do
assignee: []
created_date: '2026-10-23 18:12'
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
