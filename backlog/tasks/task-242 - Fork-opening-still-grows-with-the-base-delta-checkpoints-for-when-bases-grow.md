---
id: TASK-242
title: 'Fork opening still grows with the base: delta checkpoints, for when bases grow'
status: To Do
assignee: []
created_date: '2026-11-08 05:44'
labels:
  - perf
dependencies: []
references:
  - TASK-92.55
priority: low
type: enhancement
ordinal: 676000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Left from TASK-92.55 (the user's decision, 2026-11-08). Since its slices 1-2 (a persistent extents map, checkpoints decoded once a host), a fork's opening unpacks and reads nothing more of a larger base, but its time still grows with the base, ~0.3 us an extent in a release build: forks of a base of 32,768 extents open in ~11 ms against ~3.5 ms at 1,024 (debug 42 ms against 5 ms; Linux in Docker, test forks_opening_reads_nothing_more_of_a_larger_base). Under a round trip to a real bucket at today's base sizes, so not worth a format change now.

What grows: the parent's writer writes a checkpoint of its whole state at each fork (upload.rs acknowledged -> Writer::checkpoint: following the collector's moves for every extent, encoding every entry) and the child's recovery decodes it (recovery::state_at -> checkpoint::decode); the child's store then builds its own map of every inherited extent (extent_store Recovering::open).

The design to write into this task before coding (a format change): checkpoints that name the checkpoint they are a delta over (the branch's own previous one, or for a child's first the parent's at its fork), holding only what changed; the writer tracking what changed since its last checkpoint and following moves only for those objects whose moves changed; chains compacted past a length or size; the collector keeping every checkpoint a kept one names, and the fetcher's checkpoint lookups (Fetcher::lookup) walking the chain. Then the child's store opening with its inherited extents layered, not rebuilt.

Dropping the per-fork checkpoint instead (no format change) was weighed and set aside: each child would replay up to 64 of its parent's footers from the bucket (~6 more round trips a fork on GCS), and caching them runs into the collector's rules for segments it deleted (TASK-175, TASK-177).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A fork's opening at a base of 32,768 extents within 2x of one at 1,024 (forks_opening_reads_nothing_more_of_a_larger_base, debug), unpacking and reading as much
- [ ] #2 The delta format is written into this task and doc-7 before coding, and extent_remote's simulation (every rule test) and crates/extent_remote/mutants.sh pass on it
- [ ] #3 A fork's latency on a real bucket no worse (the perf gate's forked pair, GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
- [ ] #4 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #5 scripts/upstream-changes.py --check passes
- [ ] #6 Deviations from upstream documented where they occur
<!-- DOD:END -->
