---
id: TASK-149.14
title: >-
  Run guest/linux/fs/fxfs/dax.c in userspace, as OpenZFS runs its kernel code
  under ztest
status: In Progress
assignee: []
created_date: '2026-11-01 06:38'
updated_date: '2026-11-01 11:18'
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
- [x] #1 dax.c compiles unmodified against a shim of the kernel, with no ifdefs added for it
- [x] #2 A write, a read and a fault run through it in one process, on Linux, with no VM
- [x] #3 Locks are scheduling points of a seeded scheduler, and the same seed replays the same interleaving
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Spike done, spike/dax-userspace 0a30c4c53, merged to integrate/reliability. Verdict: build out. dax.c, device.c, fs/dax.c, fs/iomap/iter.c, xarray and genalloc compile unmodified on about 5,000 lines of shim, fake kernel and tests; a run takes about 75 ms with ASan and UBSan and a seed replays exactly. Coverage: 72% of dax.c, 62% of fs/dax.c at 300 seeds. Of 20 hand-picked past bugs put back, 11 found, 1 unreliable, 1 equivalent, 7 missed (123, patch 0008, 133, 99, 140, 44.21, 79.28). Three candidate bugs in current dax.c, none confirmed on a real kernel: a write over a hole leaving zero-page PTEs when its pieces aren't contiguous; a load reading zeros while a fault's move loops across a seal; a shared PMD read fault WARNing at fs/dax.c:458. Can't catch weak memory, real page tables, the host, or Fxfs, which is modelled. Next: guest checks for the three, CI target, tests for the misses, then real Fxfs.
<!-- SECTION:NOTES:END -->
