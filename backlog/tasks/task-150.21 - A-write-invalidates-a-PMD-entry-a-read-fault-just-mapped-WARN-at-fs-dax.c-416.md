---
id: TASK-150.21
title: 'A write invalidates a PMD entry a read fault just mapped: WARN at fs/dax.c:416'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-02 05:15'
updated_date: '2026-11-03 13:58'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 567000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-153.3: seed 2677 of zloop, 1 in 3,000; steps with dax_pause reproduce it on the base commit too. A fix in iomap_end passes the seed but not those steps; it needs work in the fs/dax.c patch series. Also the WARN behind the weak catches of TASK-123, 125 and 79.12 in guest/sim, which nobody could explain. Steps are in TASK-153.3 notes and guest/sim/README.md on refactor/open-once.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A scenario fails on the code as it stands
- [x] #2 Fixed, red with the fix out
- [x] #3 zloop clean at 3,000 seeds
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Find a failing seed at this head and trace it (mutant task-153.3-lag-under-a-pte, seed 1486 at 200).
2. Turn its steps into a guest/sim test that WARNs on main.
3. Fix in fs/dax.c (new patch 0012): __dax_invalidate_entry unmaps the entry's pages under its lock before its folio goes.
4. Mutant putting it back, README rows.
5. Guest check if reachable without new fault injection.
6. Prove: zloop 3,000 seeds and mutants.sh on GCE; DAX guest suites with a kernel built from the tree; upstream-changes.py --check.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-03.

CAUSE. invalidate_inode_pages2_range (mm/truncate.c; the sim's kernel/vfs.c alike) calls dax_invalidate_mapping_entry_sync for each clean entry, which puts the entry's folio (dax_folio_put), then unmaps the range at the end. Nothing keeps faults out meanwhile, unlike a truncate, which holds the invalidate lock. A write saying its pages new (dax_iomap_iter) so puts a PMD's folio that a PMD, or a PTE through the PMD's entry, still maps: dax_folio_put splits it under the mapping, fs/dax.c:416's WARN (folio_ref_count of a split page).
HOW SEED 1486 GOT THERE (mutant task-153.3-lag-under-a-pte, which only moves seeds): a load maps a chunk of the big file with a PMD; a store's fault of page 0 moves its block, then an injected fsync fails (-28), and fxfs_dax_map_moved's read-only MAP of the new extent says ENOENT (the device holds nothing of it), so page 0 becomes a hole in the range (fxfs_dax_unmap_pages, from 1167); the PMD entry stays. A load of another page of the row maps a PTE through that entry (folio ref 1). A write(2) of page 0 says fresh: IOMAP_F_NEW, invalidation, the WARN (traced with pr_err in __dax_invalidate_entry: ino 101 index 0 order 9 refs 1).

FIX. guest/linux/patches/0012-dax-invalidate-unmaps-under-entry-lock.patch: __dax_invalidate_entry, for an invalidation (not a truncate), locks the entry, unmaps its pages (the whole PMD row for a PMD entry), relocks the xarray, then puts the folio: as grab_mapping_entry already does for a PMD it downgrades, and dax_insert_entry for an entry it replaces. Faults wait for the entry lock, so none maps the pages between the unmap and the put. Truncates keep their order (unmap under the invalidate lock first). scripts/build-guest-kernel.sh and guest/sim/kernel-tree.sh take the patch; CI's sim tree cache is keyed on the patches.

TEST. guest/sim/tests/pmd_entry_invalidated_mapped, deterministic: 4 MiB file in two writes (a range of its own), PMD load; a write to another file takes staging blocks, and the seal's restage MAP of the new staging extent is failed (sim_device_fail), so the device holds nothing of the extent the store's fault then moves page 0 (zeros, so no device write) into; dax_pause at MOVED and SEALING: a second seal shuts the gate, the storer killed, map_moved's ENOENT leaves page 0 a hole; a load of page 1 maps a PTE through the PMD entry; write(2) of page 0. On main: PANIC, WARN at fs/dax.c:416. With 0012: passes, page 0 and page 1 read right through the mapping and read(2). Mutant guest/sim/mutants/patch-0012-invalidate-unmaps-after: caught by the scenario in 1.9 s.

BLIND SPOT. Found only by seeds, ~1 in 6,000 at 200 operations: interleave needs a fault's failed move into an extent the device doesn't hold, under a PMD entry that survives, then a write of that page. No scenario combined a PMD entry with a hole a failed move left. The class (a folio put while mapped) is now checked by the scenario, and dax_folio_put's WARN stays the oracle.
LOOKED AROUND. Every other put of an entry's folio in fs/dax.c: grab_mapping_entry's downgrade (zero/empty only, a zero PMD unmapped under its lock), dax_insert_entry's replacement (unmaps under the entry lock), dax_delete_mapping_entry and dax_delete_mapping_range (callers hold the invalidate lock and unmap first: truncate_setsize, dax_break_layout). fxfs's own invalidate_inode_pages2_range callers: fxfs_dax_release (invalidate lock held for writing, dax_layout_busy_page_range unmaps first), super.c's, for non-DAX inodes. None other.
STRUCTURE. The rule is one: a folio goes only once nothing maps it and nothing can map it before it goes. Truncation gets it from the invalidate lock, invalidation now from the entry lock: the one put site that had neither. Nothing architectural; the weak catches of TASK-79.12 and the lag mutant were this WARN coming first.
GUEST CHECK: not reachable without new fault injection (OPEN_QUESTIONS.md, decision): the steps need the restage's MAP to fail.
ALSO. mutants.sh at this change: 30 of 37 caught; TASK-79.12 now by its own deadlock (seed 2865) rather than the WARN; bdc653701 by the new scenario (a hang). Not caught in 3,000: patch-0008, task-150.14, task-153.1-fault-limit, task-153.3-refused-slice-stale, task-153.6-lag-past-the-end (all also uncaught before, per TASK-153.3), task-79.28-range-too-small (equivalent), and task-153.3-lag-under-a-pte, which 7289 200 found before 0012 and no seed 3,001-15,000 at 200 finds now: TASK-149.26.

RESULTS. GCE n2-standard-4 (us-east1-b), tree from guest/sim/kernel-tree.sh with 0012: zloop.sh -s 3000: every one-run test passes, 3,000 seeds, 0 failed, 289 s. mutants.sh -s 3000: as above. GCE fxdax-test-t15021-dax, release guest kernel built from the tree (series with 0012), fxfs-guest-dax, fxfs-guest-checks-dax, fxfs-guest-checks-dax-delay, xfstests-fxfs-guest-dax: all OK; xfstests 16 of 650 failed, the known list (generic/009 092 094 225 255 301 401 414 467 477 568 578 702 796 797 798); no WARN, no unexplained refusals. scripts/upstream-changes.py --check: passes. Workspace nextest not run: nothing of the Rust workspace changed.

Rebased on origin/main 0417d86b5 (TASK-153.4's size paths in fxfs dax.c among it): the scenario WARNs at fs/dax.c:416 without 0012; with it zloop.sh -s 3000: 0 failed, every one-run test passes (283 s); mutants patch-0012 (scenario), bdc653701 (scenario, hang), task-79.12 (seed 2865, its deadlock), task-153.3-lag-under-a-pte (not caught) as before. The DAX guest suites ran on the tree before this rebase (987ccbf46 + notes).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fs/dax.c's invalidation put an entry's folio before unmapping it, with nothing keeping faults out: a write saying its pages new split a PMD's folio under a mapping of it (WARN at fs/dax.c:416). Patch 0012 unmaps each invalidated entry's pages under its lock first. guest/sim's pmd_entry_invalidated_mapped plays the steps out: WARN on main, passes with the fix; mutant patch-0012-invalidate-unmaps-after caught by it. Verified on GCE: zloop 3,000 seeds clean, mutants.sh 30 of 37 (the 7 uncaught already known, or TASK-149.26), DAX guest suites with a kernel built from the tree OK (xfstests the known 16 of 650), upstream-changes.py --check passes. No guest check: unreachable without new fault injection (OPEN_QUESTIONS.md).
<!-- SECTION:FINAL_SUMMARY:END -->
