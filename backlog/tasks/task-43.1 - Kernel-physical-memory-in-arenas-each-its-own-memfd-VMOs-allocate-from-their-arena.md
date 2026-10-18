---
id: TASK-43.1
title: >-
  Kernel: physical memory in arenas, each its own memfd; VMOs allocate from
  their arena
status: In Progress
assignee: []
created_date: '2026-10-18 04:00'
updated_date: '2026-10-18 04:00'
labels: []
dependencies: []
parent_task_id: TASK-43
priority: high
type: feature
ordinal: 130000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The ported Zircon kernel (crates/zircon vm/pmm.rs, physmap.rs) has one physical memory, one memfd. Split it into arenas: each a memfd of its own, with pages allocated per arena, while the memory budget, pressure and eviction stay global. A VMO, and so a store's extent cache entries, takes pages from the arena it was created in. Public arena plus one per tenant.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Pages of VMOs created in different arenas are in different memfds, shown by kernel tests
- [ ] #2 Memory pressure and eviction work across arenas: one tenant's cache can be evicted for another's
- [ ] #3 Existing kernel, fxfs and fxdax tests pass with the single default arena
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
