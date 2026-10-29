---
id: TASK-119.9
title: >-
  Pause points: named steps in dax.c a check holds and releases, scripting exact
  interleavings (evacuation gather/MAP/place, seal begin, map_write vs gate,
  map_chunk extents vs gate)
status: In Progress
assignee: []
created_date: '2026-10-28 19:45'
updated_date: '2026-10-29 08:05'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: high
ordinal: 447000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Fable and Astra (2026-10-28): fxfs_dax_delay sleeps <=200 us a quarter of the time while a MAP takes ~400 us, so races are luck, and a seed cannot replay thread scheduling. A pause at a named step, released by the check, makes an interleaving exact and repeatable; counters prove the overlap happened.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A check pauses an evacuation after its gather, writes/truncates/fsyncs, releases it, and verifies contents
- [ ] #2 Each pause point has a check that reaches it (counter)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-29, branch t3/task-112-6: pause points GATHERED (an evacuation between gathering and its MAP), READ_MAPPED (a read between its iomap and its copy), EVACUEES_MAPPED (between MAP and placing), PLACED (after placing); fxfs.dax_pause_ino limits a file's steps to one inode. Checks evacuation_paused (a write meanwhile waits for the evacuation; kills evacuation_unlocked) and prune_while_paused (the page a paused read maps moved out and pruned meanwhile; kills prune_in_use). Pending GCE r214 (mutants), r215 (checks, mutant prune_in_use).

GCE r220: the mutant prune_in_use killed in round 1 by prune_while_paused alone: the region let go of under the paused read, the window unmapped it, and the read's copy was refused ('an access to a page the window maps nothing at'), the vCPU failing with EFAULT a second later. r214: evacuation_unlocked killed, evacuation_paused failing. Both survivors of TASK-44.19 now die in one round, by design rather than timing. Caveat (r215): both checks need room for regions, which TASK-128 exhausts late in the full suites.
<!-- SECTION:NOTES:END -->
