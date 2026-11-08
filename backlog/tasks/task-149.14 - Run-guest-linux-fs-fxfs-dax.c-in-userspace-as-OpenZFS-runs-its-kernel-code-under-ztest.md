---
id: TASK-149.14
title: >-
  Run guest/linux/fs/fxfs/dax.c in userspace, as OpenZFS runs its kernel code
  under ztest
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 06:38'
updated_date: '2026-11-08 06:25'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: high
ordinal: 500
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Most fixes (about 72% of 231 recent ones) are in DAX and the guest kernel, and dax.c can only run in a VM, on GCE, so its failures take hours to pin down and nothing shrinks them. OpenZFS builds its kernel code into a userspace library, libzpool, with a fake kernel underneath, and ztest stresses it in parallel, killing itself with SIGKILL as a power cut, reopening the same pool, and running on every push through zloop.sh (~/src/zfs: cmd/ztest.c, scripts/zloop.sh). Measured: dax.c is 5,259 lines making 229 distinct kernel calls, 38 of them MM and 27 VFS. User-Mode Linux can't do this: FS_DAX needs ZONE_DEVICE, which needs memory hotplug UML lacks, and it is x86-only. The code under test stays real and the kernel beneath it is fake, so locks become scheduling points, giving seeded interleavings in C, which Shuttle can't reach. Real page tables and fs/dax.c are simulated, so guests remain the check of what the shim gets wrong. Rank: above TASK-149.2 if the spike works, as its script generator then drives this too.

At dde8d58d3 (writable DAX deleted, 37c331a6c; resize, punch and clone driven from data.c since f22c9459e; a range's page state owned by pages.c since bda2e53a0): dax.c is 3,937 lines and moves no page, so the fault check (AC #7) is reset only by something other than a fault unmapping a page: a rebind's MAP over it (fxfs_dax_rebind, guest/linux/fs/fxfs/dax.c:2142-2175), an UNMAP of a range (fxfs_dax_drop_pinned, dax.c:1627-1672), a truncate or punch (fxfs_data_resize and fxfs_data_punch, guest/linux/fs/fxfs/data.c:31, 60). A writeback's write-protect (fxfs_dax_protect, dax.c:2034-2050) makes the next store fault, not a load. The sim still counts faults in one global, sim_mm_faults (guest/sim/kernel/mm.c:468, counted at 488 and 553), read by guest/sim/tests/read_mappings_stay.c alone (44, 54). Patch 0010 still exists (guest/linux/patches/0010-dax-read-replaces-no-entry-naming-its-page.patch).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 dax.c compiles unmodified against a shim of the kernel, with no ifdefs added for it
- [x] #2 A write, a read and a fault run through it in one process, on Linux, with no VM
- [x] #3 Locks are scheduling points of a seeded scheduler, and the same seed replays the same interleaving
- [x] #4 Stores carry a generation, so a stale or zero read is caught, as ztest's txg stamps catch temporal incursions
- [x] #5 Reintroducing a past DAX bug (a moved-unmapped mutant) fails
- [x] #6 It runs in CI on every push, as zloop does
- [x] #7 Every scenario and seed checks that a load faults again on a page its mapping already held only after something other than a fault unmapped it (a rebind's MAP over it, an UNMAP, a truncate or punch), counted per mapping and page in guest/sim/kernel/mm.c
- [x] #8 With patch 0010 taken out, interleave fails the fault check above, with no scenario written for it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
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

pmd_over_stale_index on real kernels, r425, r427, r429, r451: no PMD ever mapped: a guess of alignment was wrong, the chunk's moved page leaving a fault's iomap under 2 MiB. The assertion that a PMD was mapped is gone, the check's bytes stay; it no longer shows candidate 3 on a real kernel, which sim scenarios pmd_over_stale_index and read_pressure do (patch 0011). That a PMD over a stale index happens on a real kernel at all rests on the sim, the other session's read_pressure and the sandbox's hang, not on this check.

Fault budget, from TASK-152. The read-fault livelock was invisible here: each fault succeeded, so no WARN, no wrong byte, no fault going round 256 times, and the sim has no cost model. Only read_mappings_stay reads sim_mm_faults, in one scenario. Count faults per mapping and page in kernel/mm.c, reset by a move or by unmap_mapping_pages from anything but a fault, and fail the run past one.

Handed back 2026-11-02 (triage): AC6 met: ci.yml's guest-sim job runs zloop.sh -s 3000 on every push to main, passing (runs 41513855862, 41562059405). Left: AC7, count faults per mapping and page in guest/sim/kernel/mm.c (one global sim_mm_faults now), reset by a move or a non-fault unmap, failing past one; AC8, red with patch 0010 out; DoD3.

2026-11-05 triage: AC #6 checked: .github/workflows/ci.yml's guest-sim job (ci.yml:116-140), run on every push to main, runs guest/sim/mutants.sh --check and guest/sim/zloop.sh -s 3000; it passed in CI run 42135112053 on c82e2be51. Description's citations brought to dde8d58d3 (dax.c moved by f22c9459e and bda2e53a0).

2026-11-08, AC #7 and #8 (branch task-149-14). kernel/mm.c: each mapping's pages held as a fault maps them (vm_area_struct.held, a page or a PMD's row); a load (or a pin) that faults on a page its mapping holds, the page unmapped still under the mm lock, fails the run: 'a load faults again ... and nothing but a fault unmapped it'. What lets go of them: every unmap_mapping_pages but a read fault's of its own page, the one patch 0010 stops (dax_insert_entry replacing an entry naming the page), so a rebind's MAP, an UNMAP (fxfs_dax_drop_pinned, a store fault's fxfs_dax_fault_drop_slice among them), an eviction for room (fxfs_dax_release from fxfs_dax_alloc_window), a truncate or punch; and a shared PMD's split by a write fault (wp_huge_pmd's, by design). The rule took three rounds, each a false alarm traced to dax.c's call: (1) 'any unmap inside a fault' flagged a store fault evicting another range for window room (dax.c:1288 via fs/dax.c:788; 69 of 600 runs); (2) 'a fault's unmap of its own page' flagged a store fault dropping the slice it can't store to (dax.c:3485 -> 1781 -> 1288; seed 834 and the sweep's cow_store_to_pinned_slice at calls 34-37); (3) a load found the page unmapped, then another task's fault mapped and held it while it waited for the mm lock (seed 23): the check now asks under the lock. With patch 0010 in, no violation on main.
AC #8: interleave mapped each file once in one mm, so no page ever had two mappings of its file; each worker now maps the shared files itself (slots.addrs[WORKERS]), as processes sharing a binary do, and loads and stores through its own. Mutant guest/mutants/patch-0010-reads-replace-their-entries (patch 0010 reversed, Kernel: sim, Killer: interleave): killed at seed 610 (seed 197 under an earlier rule), with no scenario written for it.
Runs (Docker, guest/sim/linux.sh): zloop.sh -s 3000: 0 of 6,000 runs failed, every test of one run passes; sweep.sh: passes; mutants.sh: every sim mutant killed but the six none; mutants.sh --check 43/0, scripts/guest-mutants-gce.py --check 55/0, fxfs-sim-diff mutants --check. Rust workspace not run: no Rust changed.

2026-11-08: land-h10 on main as afee06cd5 (land-all4, 722ce0723), CI 42449438764 (land-all4, on main as 722ce0723) green: test (workspace) on Linux, macos (workspace), guest-sim, guest-sim-sweep, scripts. DoD 3: guest/sim is fxdax's own, no upstream to deviate from.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
guest/sim runs dax.c unmodified over a kernel shim with a seeded scheduler, generation-stamped stores, mutants and CI (AC 1-6, earlier). AC 7-8 (afee06cd5): kernel/mm.c holds each mapping's pages as a fault maps them and fails a load faulting on a page held that only a fault unmapped; interleave's workers map the shared files themselves; with patch 0010 out (mutant patch-0010-reads-replace-their-entries) interleave fails at seed 610. Verified: zloop 3,000 seeds, sweep, every sim mutant killed, CI 42449438764.
<!-- SECTION:FINAL_SUMMARY:END -->
