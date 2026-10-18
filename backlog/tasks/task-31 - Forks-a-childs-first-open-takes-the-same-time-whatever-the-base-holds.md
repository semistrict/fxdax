---
id: TASK-31
title: 'Forks: a child''s first open takes the same time whatever the base holds'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-17 15:26'
updated_date: '2026-10-18 19:16'
labels:
  - performance
  - forks
dependencies: []
priority: high
ordinal: 100000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Forking takes the same time whatever the parent holds (TASK-28.2: Lineage::fork writes the child's record only), but a child's first opening (Store::open_in, unresolved branch) recovers the parent's state at the fork (state_at, parsing its index file) and writes a checkpoint of all of it into the child's own index: time and space proportional to the base. With a few hundred children forked from one base image per host, that is a few hundred full copies of the base's index, on disk and in memory (each Store holds its own HashMap of every extent), and a disk I/O storm when many start at once. Lineage::open also recovers every branch's state to compute claims. Scope: the disk image only (extent_store, extentd).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A fork's first opening reads and writes the same bytes whatever its parent holds, shown by a test of parents of different sizes
- [ ] #2 Children of one base opened in one process share one copy of the base's index in memory: memory per open child grows with what the child changed, not with the base, shown by a test
- [ ] #3 Crash recovery, rebuild from checkpoints and footers, deleting branches, and claims on files work as before, shown by the existing store and crash tests
- [ ] #4 Opening a store with hundreds of forks of one base takes time that does not grow with the base's size per fork, measured
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_store layers.rs: Layer (immutable, shared: extents and tombstones over the layer below, with sums kept as differences: stored, per-segment live, extents per file) and Extents (a branch's: a layer it is over, what it changed, what of the layer it no longer holds).
2. Checkpoints of a fork hold what it changed over its parent's state at the fork, with "gone" rows; BranchRecord gains layered (old records parse as not layered).
3. Store::fork freezes the branch's state in its commit: the layer the child is over, which the parent goes on over.
4. Lineage: forks hold their layer's files (Inherited: holders, and the files each let go of); claims of their own as before; layers built at open from each index file once for all forks of it (layers_at); parents keep index files their forks are over, up their lines.
5. Store::open_in opens a fork over its layer: tracks only the segments of what it changed or hid; inherited segments' files kept apart; a root branch shares its latest fork layer (rebase).
6. Tests: first opening's bytes independent of the parent; footprint of forks; Lineage::open bytes per fork independent of the base; layer unit tests; existing store, crash and shuttle tests.
<!-- SECTION:PLAN:END -->
