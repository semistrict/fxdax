---
id: TASK-108.1
title: >-
  extentd at its memory cap: VMs initialise 2.6-3.8x slower and run at half the
  tps, worse with each VM
status: To Do
assignee: []
created_date: '2026-10-25 19:06'
updated_date: '2026-10-25 19:23'
labels:
  - bug
dependencies: []
parent_task_id: TASK-108
priority: high
ordinal: 408000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox report round 5: with one 8 GiB daemon serving ten pgbench VMs one after another, the VMs started once it reached its cap took 53-77 s to initialise (19-21 s below it) and ran 407-547 tps (991-1,080 below), each slower than the last. A busy host always sits at the cap (TASK-103 keeps the cache warm up to it by decision), so these are the numbers production sees. Leads unmeasured: evict_least_used sorts every cached extent (up to 65,536) on each relief and evicts whole extents; an extent's recency is stamped only on cache lookups, not as windows fault its pages, so a base extent every fork maps may look oldest and be evicted while in use, its pages fetched again (25 ms from the bucket).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A VM started on a daemon at its memory cap initialises and runs pgbench within 10% of one started on a fresh daemon (FXDAX_PGBENCH_VMS=10 on GCE)
- [ ] #2 Where the time at the cap went is recorded, with counters or stacks
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Reading (unmeasured): at the cap each relief pass frees RELIEF_BYTES, 4 MiB (pmm/relief.rs), and allocations wait for it; extents::evict_least_used, the pass's evictor, gathers and sorts every cached extent of the arena (up to 65,536) under the cache's lock, then evicts whole extents, all their pages, in order of their last cache lookup, not of their pages' use. So paging in N MiB at the cap costs N/4 passes, each slower as entries accumulate with each VM's extents: matches each VM being slower than the last. r102 (FXDAX_PGBENCH_VMS=10, 8 GiB) reproduces.
<!-- SECTION:NOTES:END -->
