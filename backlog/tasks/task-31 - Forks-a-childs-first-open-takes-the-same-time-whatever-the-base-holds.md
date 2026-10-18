---
id: TASK-31
title: 'Forks: a child''s first open takes the same time whatever the base holds'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 15:26'
updated_date: '2026-10-18 20:21'
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
- [x] #1 A fork's first opening reads and writes the same bytes whatever its parent holds, shown by a test of parents of different sizes
- [x] #2 Children of one base opened in one process share one copy of the base's index in memory: memory per open child grows with what the child changed, not with the base, shown by a test
- [x] #3 Crash recovery, rebuild from checkpoints and footers, deleting branches, and claims on files work as before, shown by the existing store and crash tests
- [x] #4 Opening a store with hundreds of forks of one base takes time that does not grow with the base's size per fork, measured
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Verified: macOS 2048 tests and the Shuttle suites; GCE 2096 tests, the Shuttle suites, and 18 guest cases (local buckets and GCS). AC1: forks_open_in_the_same_bytes_whatever_their_parent_holds (200 and 800 extents: the first opening reads 0 bytes and writes the same; the next opening reads and writes the same). AC2: forks_share_their_parents_state (each fork's footprint is its open segment and nothing inherited; 2 entries, 2 segments after a write and a free; the parent's 0). AC3: the store, crash and shuttle tests; one expectation changed with the design: a parent keeps its index file as of a fork while the fork lives (forks_not_yet_opened_keep_what_they_inherited). The crash test caught a bug on the way (a fork required the file of an inherited segment it had let go of), fixed. AC4: stores_open_their_forks_in_bytes_independent_of_their_base: Lineage::open and opening 50 more forks read the same bytes for bases of 200 and 800 extents (measured in bytes read, which is deterministic, rather than time). Tests run on GCE, not Lima (project rule).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A fork's state is its parent's at the fork, a layer (extent_store layers.rs) the parent's forks of that point share with the parent, one copy in memory; each branch holds what it changed over it. A fork's index holds what it changed (checkpoints with gone rows; records flag layered, old records parse as before), so opening a fork reads and writes what it changed, whatever the parent holds, and tracks only the segments of what it changed or hid. The lineage holds files through layers (Inherited), builds each layer at open from an index file read once for all forks of it, and keeps parents' index files while forks are over them. Verified by four new tests and the existing store, crash and Shuttle tests on macOS and GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
