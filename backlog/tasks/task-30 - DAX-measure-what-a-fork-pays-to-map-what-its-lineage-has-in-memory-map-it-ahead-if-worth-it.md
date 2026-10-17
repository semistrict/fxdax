---
id: TASK-30
title: >-
  DAX: measure what a fork pays to map what its lineage has in memory; map it
  ahead if worth it
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-17 15:04'
updated_date: '2026-10-17 16:19'
labels:
  - performance
  - dax
dependencies: []
priority: medium
ordinal: 99000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Idea from the user (2026-10-17): extents of a lineage that other VMs already have in memory could be mapped into a new fork's DAX window ahead of its guest asking, so that a fork starts warm.
What already holds: sealed extents are cached once in extentd's kernel, keyed by extent ID, and every window maps the same pages (crates/fxdax/src/extents.rs); a window's MAP maps the pages already in memory at once (MAP_RANGE in crates/fxdax/src/dax.rs) and, on KVM, pre-faults them for the guest (Prefault, TASK-25.3). So a fork reading what its parent read already does no I/O and takes no per-page fault for resident pages.
What a fork still pays: a MAP request, guest to extentd and back, for each extent it maps; and faults, and reads, for pages not in memory yet.
Mapping the whole lineage into a fork eagerly has costs: the window's layout is the guest's (Fxfs chooses offsets), so the host cannot place mappings without a protocol or a fixed layout; forks would no longer take O(1) (TASK-28.2); windows are 2 GiB; every mapped page has a mapping in every window, which reclaim must tear down; page tables cost ~2 MiB per GiB mapped per VM; mappings of extents a fork frees go stale. Lighter options: a profile of the extents a lineage's VMs touch at boot and warm-up (doc-7, Reading), which a new fork's host reads into memory ahead, and which the guest's Fxfs maps at mount; or batching MAP requests.
Decide only once measured: if MAP round trips and faults are a small part of a fork's start and pgbench time, nothing is needed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A fork's boot, and pgbench in it, on GCE, are measured with a parent warm: MAP requests and their time, DAX faults and their time, and pages read from the store or the bucket, recorded under docs/measurements
- [ ] #2 The decision to map ahead, and how (none, profiles read ahead, guest-side mapping at mount, batched MAPs), is recorded in this task with the measurement behind it
- [ ] #3 If mapping ahead is done: a warm fork's boot and pgbench are faster on GCE than without it, shown by the fs bench, with fork creation still taking the same time whatever the lineage holds
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extentd: FORK (forks of stores the daemon serves, which it can open) and STATS (process-wide counters: MAPs and their time, DAX faults and their time, page-ins and bytes from the store or bucket, READs and bytes from the store).
2. Bench: bench-fork-fxfs-guest-dax-{warm,cold} in libkrun's fs bench: a base VM makes a copy of /usr and a pgbench database; each iteration forks it through extentd and boots a VM on the fork (mount, read /usr whole, start PostgreSQL, pgbench -S), served by the daemon that has the base's data (warm) or a fresh one (cold), reporting extentd's counters per fork.
3. Measure on GCE; record the decision; build what the measurement justifies, with the bench showing its effect.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
User asked (2026-10-17): implement with perf tests that show how much difference it makes for a relevant workload. Work in worktree ~/src/fxdax-premap, branch premap.

Lima, 2 iterations (indicative only): warm fork: read /usr 14.6 s, 0 page-ins, 97 faults (1.2 ms), 370 MAPs (~35 ms in extentd, ~0.2 s seen by the guest), pgbench -S 3.2k tps. Cold fork: read /usr 38.4 s, 4.7k page-ins (610 MB from the store), 11.8k faults (5.9 s), 370 MAPs (~0.3 s seen by the guest), pgbench -S 3.5k tps. So what a fork pays is pages not in memory, not MAP round trips. Pushed: 17d5a4c (extentd FORK/STATS, counters), 1ccbb25 (bench, libkrun 0c84634). GCE run on fxdax-bench-fork in progress.

GCE (c3-standard-8, us-east4, 3 iterations; docs/measurements/gce-fork-bench-2026-10-17T1609Z): base = a copy of the guest's /usr (2.8 s) and a pgbench -s 10 database (11.5 s), 193 MB stored (compressed). Warm fork vs cold: mount 0.19 vs 0.20 s; read /usr whole 0.57 vs 6.16 s (10.8x); PostgreSQL start 0.15 vs 0.25 s; pgbench -S 46.7k vs 20.6k tps. Cold: 9.7k page-ins, 1.27 GB read and decompressed from the store, 37.9k DAX faults (13.9 s of fault time across threads). Warm: 0 page-ins, 117 faults (0.7 ms), 381 MAPs (40 ms in extentd, 259 ms as the guest sees them, since a MAP of resident pages maps and pre-faults them at once; 114 ms cold, where it maps nothing yet). READs: ~600, 4 MB, from the store in both (TASK-32).
Decision: do not map ahead. Mapping the lineage's resident extents into a fork before its guest asks would save at most the MAP round trips, ~0.26 s of a warm fork's run and nothing of a cold one's, and needs a protocol or fixed layout for the guest's window. What a fork pays is data not in memory: 10.8x on reading the base's files, 2.3x on pgbench -S. The levers are getting the base's data into memory before the fork reads it (a boot profile read ahead into the extent cache, the cold fork's page-ins made larger so that each decompressed 1 MiB part is supplied whole rather than 128 KiB of it, READs from the cache: TASK-32), each measurable with these cases.
<!-- SECTION:NOTES:END -->
