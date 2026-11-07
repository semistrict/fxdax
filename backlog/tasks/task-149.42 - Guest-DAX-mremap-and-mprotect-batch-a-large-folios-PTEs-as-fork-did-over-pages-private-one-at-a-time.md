---
id: TASK-149.42
title: >-
  Guest DAX: mremap and mprotect batch a large folio's PTEs, as fork did, over
  pages private one at a time
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-07 10:24'
updated_date: '2026-11-07 20:17'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 658000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-149.16 found fork copying a large DAX folio's PTEs as one, all writable if any was (6.18's copy_present_ptes, FPB_MERGE_WRITE): a chunk read in whole (a PMD, so its window pages one folio), split by a store, gave a forked child writable PTEs of pages no PRIVATE had made the VM's, and the host refused its store till the VM went down (GCE r861). Fixed by dropping VM_MIXEDMAP, so fork copies no PTEs of Fxfs's mappings. Other paths batch a large folio's PTEs too and may carry one PTE's write bit to its neighbours: mremap's move_ptes (folio_pte_batch, set_ptes of the first PTE's bits, 6.17) and mprotect's change_pte_range (6.17's batching, MM_CP_TRY_CHANGE_WRITABLE). Copying on write makes each page private on its own, so a folio's pages differ in writability, which those paths assume they don't. The hostile scripts' Mremap moves a mapping made for its stores, not the kept one; nothing calls mprotect.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A guest check moves (mremap) and mprotects a mapping of a chunk read in whole, split by a store, then stores through the result to a page it only read: the store faults and makes the page private, or the check fails as r861 did
- [ ] #2 Fxfs keeps a chunk's folio from mixing writable and read-only PTEs, or each path is shown not to merge write bits, with the kernel's code cited
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Read 6.18's batching paths: which may set a write bit on PTEs of a large folio that differ in it (copy_present_ptes, change_pte_range, move_ptes; the rest only clear or read).
2. Guest checks after a split PMD (SplitChunk, shared with the fork check): mremap the mapping, mprotect it read-only and writable again; then store to a page only read: it must fault and become private.
3. Hostile scripts: ops that read the kept mapping whole, mprotect it or move it, then store through it.
4. GCE red on the unfixed kernel (r932); the narrowest fix if a path carries a write bit; GCE green (r933), the mutant taking the fix away killed (r934).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Code read (6.18.54): move_ptes batches with mremap_folio_pte_batch's FPB_RESPECT_WRITE, so a batch's PTEs agree in write bit, set_ptes of the first's bits (dirty and young merged, not write): mremap carries none. change_pte_range batches with FPB_RESPECT_SOFT_DIRTY | FPB_RESPECT_WRITE, not dirty; modify_prot_start_ptes (get_and_clear_full_ptes) merges the batch's dirty bits into the first; with MM_CP_TRY_CHANGE_WRITABLE, can_change_shared_pte_writable's pte_dirty of that one makes the whole batch writable (prot_commit_flush_ptes). The guest's x86 config has no MEM_SOFT_DIRTY, so soft-dirty splits no batch. Others (zap, rmap's unmap, madvise, mlock, mempolicy, khugepaged, which skips DAX) only clear or read PTEs.
GCE r932 (unfixed): cow_fork_after_a_split_pmd and cow_mremap_after_a_split_pmd pass; cow_mprotect_after_a_split_pmd took the VM down (KVM_RUN EFAULT 5288 times in 1 s, AccessRefused): red. fxfs-guest-fsx-dax 300 s passed with the first hostile ops (mprotect alone), which needed a chain of ops to reach it; the ops now read the mapping whole and store after.
Fix (57bf1ea50): guest/linux/patches/0013, change_pte_range adds FPB_RESPECT_DIRTY to a shared mapping's batch made writable, each PTE decided by its own dirty bit as before 6.17's batching; documented in the patch's header. Mutant task-149.42-mprotect-batches-whatever-their-dirtiness leaves 0013 out of the series (killer: the suite, the VM going down). Host note: XFS on Linux 7.2.9 failed an O_DIRECT read near the end with EIO on hostile seed 3, with or without the new ops (the default seed passes).

GCE r933 lost (its host's SSH timed out mid-build; mm/mprotect.o had compiled). r934: the mutant leaving 0013 out killed: fxfs-guest-checks-dax passed up to cow_mremap_after_a_split_pmd, then cow_mprotect_after_a_split_pmd took the VM down (AccessRefused). r935 (the fix in): fxfs-guest-checks-dax passes (77 s; cow_fork, cow_mremap, cow_mprotect_after_a_split_pmd each pass), fxfs-guest-fsx 316 s, -dax 311 s, -delay-dax 378 s pass, the hostile scripts' new ops among them.
<!-- SECTION:NOTES:END -->
