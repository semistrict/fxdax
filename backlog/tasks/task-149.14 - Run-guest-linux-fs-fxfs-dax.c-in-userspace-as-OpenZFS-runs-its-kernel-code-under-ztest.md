---
id: TASK-149.14
title: >-
  Run guest/linux/fs/fxfs/dax.c in userspace, as OpenZFS runs its kernel code
  under ztest
status: In Progress
assignee: []
created_date: '2026-11-01 06:38'
updated_date: '2026-11-01 20:32'
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
- [x] #4 Stores carry a generation, so a stale or zero read is caught, as ztest's txg stamps catch temporal incursions
- [x] #5 Reintroducing a past DAX bug (a moved-unmapped mutant) fails
- [ ] #6 It runs in CI on every push, as zloop does
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Spike done, spike/dax-userspace 0a30c4c53, merged to integrate/reliability. Verdict: build out. dax.c, device.c, fs/dax.c, fs/iomap/iter.c, xarray and genalloc compile unmodified on about 5,000 lines of shim, fake kernel and tests; a run takes about 75 ms with ASan and UBSan and a seed replays exactly. Coverage: 72% of dax.c, 62% of fs/dax.c at 300 seeds. Of 20 hand-picked past bugs put back, 11 found, 1 unreliable, 1 equivalent, 7 missed (123, patch 0008, 133, 99, 140, 44.21, 79.28). Three candidate bugs in current dax.c, none confirmed on a real kernel: a write over a hole leaving zero-page PTEs when its pieces aren't contiguous; a load reading zeros while a fault's move loops across a seal; a shared PMD read fault WARNing at fs/dax.c:458. Can't catch weak memory, real page tables, the host, or Fxfs, which is modelled. Next: guest checks for the three, CI target, tests for the misses, then real Fxfs.

2026-11-01, branch test/sim-next, on 7082ef35d. Candidate 1 confirmed on a real kernel by GCE r395, r396 (page 1 through the mapping is stale); its fix is in the user's branch, so fixes/hole-pieces.patch goes when that merges, and zloop.sh fails until it does.

Guest checks, compiled for guest and host, not run: load_during_move (candidate 2), pmd_over_stale_index (candidate 3), sealed_piece_reach (candidate 4, needs a lockdep kernel). Candidate 3 went by a route fixes/shared-pmd.patch didn't cover, a range's own space with a page relocated; the fix is now one line in fs/dax.c's dax_folio_put (dead-ends/shared-pmd-falls-back.md).

Diagnostics triaged: dax_shadow and dax_check_places are dax.c's diagnostics being wrong, on a real kernel too, not the fake. dax_shadow: 49 of 900 seeds, all an evacuation's MAP of own-space pages a move left, whose PTEs are gone; a new check of writable PTEs under a read-only MAP or UNMAP stayed quiet over 1,500 seeds at 1,000 operations. dax_check_places: size read, then Fxfs asked, a truncate between; super.c has that order, and fxfs-guest-checks-dax-delay sets it, so a guest run can fail on it. Candidate fixes in guest/sim/fixes; dax.c untouched.

AC4: stores carry stamps (file, place, worker, generation, seal). Own reads exact, others' bounded, zeros only over a store; Fxfs's extents checked after fsyncs. TASK-122 now fails as 'a stale read', TASK-133 as 'a zero read'. AC5: 18 of 20 past bugs show, at most 3,000 seeds, against 11 of 20; the other two equivalent. AC6: guest/sim/zloop.sh and CI's guest-sim job, unpushed so unrun in CI: run here from kernel-tree.sh's tree on arm64, and on x86-64 under emulation with clang 19, not ubuntu-24.04's clang 18.

The misses. TASK-123 and patch 0008: blind spot, a chunk in one extent is a slice, so no range of its own ever had a PMD: huge_cycle now writes one in pieces, and a second mapping faults its pages again. TASK-133: the fake's Fxfs never failed: transactions now do. TASK-99: the fake never had an extent read sealed then open: device writes now open what no seal found held, as guest/rust's device does. TASK-140, TASK-44.21: no timing reaches them (dead-ends/sim-timing-reaches-reflink-races.md): a reflink does, as Fxfs says shared blocks of an open extent aren't open. Clones added, tests/open_extent_read_only and tests/sealed_piece_reach; 140 needed a check that a fault goes round at most 256 times, and 44.21's mutant takes out the fix's check too. TASK-79.28: equivalent since TASK-147; task-79.28-before-147 shows at seed 31. TASK-79.104: equivalent since TASK-101. TASK-101 had been found only by the dax_check_places false alarm: now a store the host refused, with own-space pieces (dax_fail_moves). TASK-125 at seed 2327 by an unrelated WARN: fix 2 masks most of it.

New candidates, none confirmed on a real kernel. 4: writable MAP outside the gate, a sealed piece of a writable region, through a reflink: tests/sealed_piece_reach (one task, fails on dax.c as it stands), seed 13464 at 1,000 operations without the fix. 5: SIGBUS on a range with no moved: fxfs_dax_place does nothing there, so TASK-124's relocation is skipped: seed 8083 at 200 operations without the fix.

17,000 seeds pass with all fixes (a further 3,000 of the same in zloop). Unexplained: TASK-123, TASK-125 and TASK-79.12 all show as fs/dax.c:416's WARN, a page referenced as a PMD's folio splits, at seeds near 2,000, which no base seed shows. Second mappings, reclaim under failure, and followers remain thin.

Correction to the above: that TASK-101 was first found by the dax_check_places false alarm is a guess. The first round logged only 'diagnostic WARN'; all that's known is that it went unseen once that fix was in, and shows now as a store the host refused. Also not rerun on the final workload: -s, and coverage. dax_check_places is off in runs that fail a MAP, as an evacuation can fall between the failure and the write's drop of its range.

Candidate 2 confirmed on a real kernel: r425 load_during_move: a load through a PTE during a fault's move read 0, the file holding 81. Fixed in dax.c from moved-before-mapped.patch, unmapping only when blocks moved, the patch doing so on every iomap, a cost on the write path. r425's pmd_over_stale_index proves nothing: 'no fault mapped a PMD's 2 MiB', its setup not reaching a PMD. Candidate 3 stays unconfirmed.

r429, every fix merged: fxfs-guest-script-full and -full-dax passed 2 rounds each, the first time; load_during_move passes, so candidate 2's fix holds on a real kernel; no unexplained refusals. pmd_over_stale_index never tested candidate 3: mmap's address wasn't 2 MiB aligned, so no PMD could be mapped, and it failed every run on its setup. Fixed: it maps at a chunk's alignment. Its result is not yet seen.

Correction to the note above: that the address's alignment is the cause is a guess, not shown: test_mapped_after_write maps with no address and gets PMDs. Another cause would be that a relocated page rules a PMD out on a real kernel, where the sim allows it. r451 shows which.
<!-- SECTION:NOTES:END -->
