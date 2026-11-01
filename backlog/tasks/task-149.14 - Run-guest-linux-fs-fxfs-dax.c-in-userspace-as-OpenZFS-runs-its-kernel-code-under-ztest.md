---
id: TASK-149.14
title: >-
  Run guest/linux/fs/fxfs/dax.c in userspace, as OpenZFS runs its kernel code
  under ztest
status: To Do
assignee: []
created_date: '2026-11-01 06:38'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: high
ordinal: 500
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Most fixes (about 72% of 231 recent ones) are in DAX and the guest kernel, and dax.c can only run in a VM, on GCE, so its failures take hours to pin down and nothing shrinks them. OpenZFS builds its kernel code into a userspace library, libzpool, with a fake kernel underneath, and ztest stresses it in parallel, killing itself with SIGKILL as a power cut, reopening the same pool, and running on every push through zloop.sh (~/src/zfs: cmd/ztest.c, scripts/zloop.sh). Measured: dax.c is 5,259 lines making 229 distinct kernel calls, 38 of them MM and 27 VFS. User-Mode Linux can't do this: FS_DAX needs ZONE_DEVICE, which needs memory hotplug UML lacks, and it is x86-only. The code under test stays real and the kernel beneath it is fake, so locks become scheduling points, giving seeded interleavings in C, which Shuttle can't reach. Real page tables and fs/dax.c are simulated, so guests remain the check of what the shim gets wrong. Rank: above TASK-149.2 if the spike works, as its script generator then drives this too.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 dax.c compiles unmodified against a shim of the kernel, with no ifdefs added for it
- [ ] #2 A write, a read and a fault run through it in one process, on Linux, with no VM
- [ ] #3 Locks are scheduling points of a seeded scheduler, and the same seed replays the same interleaving
- [ ] #4 Stores carry a generation, so a stale or zero read is caught, as ztest's txg stamps catch temporal incursions
- [ ] #5 Reintroducing a past DAX bug (a moved-unmapped mutant) fails
- [ ] #6 It runs in CI on every push, as zloop does
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
