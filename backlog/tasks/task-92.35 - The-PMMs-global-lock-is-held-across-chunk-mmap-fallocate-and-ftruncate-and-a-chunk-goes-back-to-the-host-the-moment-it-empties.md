---
id: TASK-92.35
title: >-
  The PMM's global lock is held across chunk mmap, fallocate and ftruncate, and
  a chunk goes back to the host the moment it empties
status: In Progress
assignee: []
created_date: '2026-10-22 21:06'
updated_date: '2026-10-24 12:29'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 310000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
An emptied chunk is released at once (pmm.rs:532-533), under the one PmmNode mutex that free_list holds across a whole page list (689-711). Releasing mmaps over a populated 2 MiB mapping and punches a hole under the arenas write lock (physmap.rs:309-332), which also blocks window installs (mmu.rs:220). Taking a chunk mmaps and ftruncates under the lock too, and nothing keeps emptied chunks. Tearing down a fork with 1 GiB cached holds the lock for 50-100 ms in total, and under a cap chunks cycle between release and re-take.

Where: crates/zircon/src/vm/pmm.rs:319-342, crates/zircon/src/vm/pmm.rs:350, crates/zircon/src/vm/pmm.rs:389, crates/zircon/src/vm/pmm.rs:532-533, crates/zircon/src/vm/pmm.rs:689-711, crates/zircon/src/vm/physmap.rs:143-145, crates/zircon/src/vm/physmap.rs:204-207, crates/zircon/src/vm/physmap.rs:259-268, crates/zircon/src/vm/physmap.rs:309-332, crates/zircon/src/arch/host/mmu.rs:220.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present after a79f138/cbe6c42 moved arenas into pmm/arenas.rs. Arenas::put releases a chunk the moment chunk_used hits 0 (crates/zircon/src/vm/pmm/arenas.rs:209-214 -> release 368-385). That calls physmap release_chunk: mmap, then fallocate PUNCH_HOLE under objects.write (physmap.rs:302-333). take/give -> assign_chunk does take_slot (ftruncate under objects.write, physmap.rs:142-153) and an mmap (248-270). All of this runs under the PMM node lock, and free_list holds it across the whole list (pmm.rs:446-475). Window installs read arena_fd under objects.read (arch/host/mmu.rs:259). Only a pre-taken spare exists; emptied chunks are not kept. Not tracked. Related: TASK-50.3, TASK-26.18.3, TASK-79.37, TASK-79.36, TASK-86.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [ ] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Finalization check (after the push of 47b7a348): not done. AC2 is not fully proven, for three reasons. (1) pmm-bench ran only on macOS, where release_chunk is a single mmap with no fallocate PUNCH_HOLE. The Linux half of the cost and of the change (the punch, formerly under the objects write lock that window installs read) has no before or after measurement. The reviewer's should-fix asks for pmm-bench on GCE at 2dac7795 (before) and e4bb83d6 (after), and scripts/example-gce.sh can now do that (9f350557), but no GCE run exists. (2) The task's second cost, chunks going back the moment they empty so that an arena under a cap cycles them between release and re-take, was not changed, and no measurement shows it before or after (e4bb83d6 'Not here'; round1 'left' item 2). (3) The take path's mmap and ftruncate still run under the PMM lock, once per 512 allocations, and this is unmeasured. Audit: I agree it is not done, and the proposal claims only AC1. AC1: the 2dac7795 message has a before measurement on macOS (346-528 other allocations, p99.9 489-891 us). AC2 lacks: (1) any GCE or Linux pmm-bench before (2dac7795) and after (e4bb83d6). I found no GCE example run in the scratchpad, so the Linux punch's cost and its move off the objects lock are unmeasured. (2) Any change to, or measurement of, chunks going back as soon as they empty and churning under a cap. e4bb83d6 says 'Not here'. (3) The take path's mmap and ftruncate, still under the PMM lock and unmeasured. The tests named (arenas chunks_going_back_..., chunks_the_host_does_not_take_back_stay_their_arenas, and the three Shuttle tests) PASS on Linux and macOS. DoD1-3 hold: pmm.rs, which is ported, has fxdax: markers on the deviation entry, NodeGuard and wait_for_chunks_going_back; locked() has none (nit).

Handed over (2026-10-24): change on main. Left: pmm-bench on GCE before and after (macOS only so far).
<!-- SECTION:NOTES:END -->
