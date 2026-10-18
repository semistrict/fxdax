---
id: TASK-43.1
title: >-
  Kernel: physical memory in arenas, each its own memfd; VMOs allocate from
  their arena
status: Done
assignee: []
created_date: '2026-10-18 04:00'
updated_date: '2026-10-18 04:39'
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
- [x] #1 Pages of VMOs created in different arenas are in different memfds, shown by kernel tests
- [x] #2 Memory pressure and eviction work across arenas: one tenant's cache can be evicted for another's
- [x] #3 Existing kernel, fxfs and fxdax tests pass with the single default arena
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
e658b24: physical memory in 2 MiB chunks, each assigned to one arena (its own memfd) at a time; per-arena free lists; chunks return when empty; global free count/pressure/eviction; starved arenas wait for a chunk; VMOs take the arena current at creation (pmm::with_arena); supplied cross-arena pages are copied; MMU/second stages/STAGE_MAP carry (arena, offset). Tests in arena_unittest (5). macOS 2001 / Linux 2029 / 8 Lima guest suites.
<!-- SECTION:FINAL_SUMMARY:END -->
