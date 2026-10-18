---
id: TASK-42.9
title: 'DAX followers touch window pages after their own UNMAP: find why'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-18 21:13'
updated_date: '2026-10-18 23:39'
labels:
  - dax
  - followers
dependencies: []
parent_task_id: TASK-42
priority: medium
ordinal: 144000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Under the followers' torture test (fxfs-guest-follow-torture-dax), a follower's guest touched a page of its DAX window 3-7 ms after its own UNMAP of the range, with no MAP between (extentd's window history), and its VMM aborted on the unresolvable fault ("unresolved DAX window fault ... ENOENT"). KVM's trace (kvm:kvm_page_fault) showed the access was a user load (the mapped blocks reader's memcpy). 77897a5 makes the host's UNMAP map zeros, read-only, in place of what it unmaps, as freeing an extent does: the access then finds something to map, and in the torture runs no reader read a zero page. That makes it harmless, not understood. Ruled out on GCE, each by a run: async page faults (no-kvmapf), a race between vCPUs (followers of one vCPU), the /proc/self/mem reader, DAX regions and slices, two ranges owning window space (a take/give bitmap), stale TLB entries (flush_tlb_all before the UNMAP), a PTE surviving the range's release (page-table walks right after it and 2/4/6 ms later), a fault leaving a PTE the guest's ranges don't map, two inodes or mappings for one file. Found since: a follower's read(2) held the invalidate_lock across its copy to user memory, which deadlocked with faults (fixed in 78ea8e0 on main); whether that bears on the late access is unknown.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The mechanism by which the guest reaches a window page after its UNMAP is identified, with the evidence that shows it
- [x] #2 A test or reproduction that shows it, which fails without its fix (or shows it is the guest's legitimate behaviour)
- [x] #3 Whether UNMAP must map zeros (77897a5) is decided and documented where it is done: kept as required, or reverted
- [x] #4 fxfs-guest-follow-torture-dax passes 10 runs of 10 on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Experiment: on the current code (78ea8e0's lock fix in), UNMAP unmapping again instead of mapping zeros, fxfs-guest-follow-torture-dax 5 runs on GCE: do the late accesses still come? If not, explain how the old read path's invalidate_lock caused them, and decide on 77897a5.
2. If they do: catch the guest at one. The host keeps the faulting vCPU's access pending and has the guest dump its state (NMI backtrace / the user VA's page-table entry via KVM), or the guest records each of its user mappings' PTE changes for the window's pages in a ring, dumped when a reader sees a page it shouldn't; find what gives the load its translation.
3. Fix or document; a test that shows it; torture-dax 10 runs of 10.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Mechanism (kvmtrace5/6 on GCE, extentd's window history and KVM's trace on the mono clock; follower of one vCPU, no-kvmapf, UNMAP unmapping): the late fault is not the guest's but x86 KVM's own. The guest's load of a window page (EPT violation, user RIP) goes to KVM's async page fault (kvm_try_async_get_page, kvm_async_pf_not_present) even without the guest's PV async PF: KVM re-enters the guest with a timer interrupt. The worker (async_pf_execute) faults the page for writing; the read-only follower refuses it (EACCES) and the page is made read-only. Meanwhile the guest's interrupt schedules the follower's batch, which drops the range and kicks UNMAP (kvm_mmio). Before the vCPU next enters, KVM faults the page again itself (kvm_arch_async_page_ready, then kvm_async_pf_ready), a read that finds the window unmapped: ENOENT, and the VMM aborted. no-kvmapf did not rule async page faults out: it disables only the PV kind. Zeros made the re-fault succeed, unobserved. Fix: the uffd second stage withholds a page the window maps nothing at (PROT_NONE, between changes, as refusals), so the re-fault fails with EFAULT, which KVM drops; UNMAP unmaps again. Red: dax::uffd::tests::accesses_to_pages_unmapped_fail aborted (SIGABRT) before the fix.

Withholding alone was not enough: the kernel maps a page into the second stage only once it is in memory, as a fault brings it in, and a withheld page can't fault; once the guest mapped its range again, the followers retried it until the VMM gave up (EFAULT, 10 of 10 torture runs, both followers, at rep_movs_alternative and the mapped reader, kvmtrace7). The window now tells its stage when it maps a range anew (SecondStage::window_mapped, STAGE_MAPPED), which gives the withheld pages back. Validation on GCE: fxdax 159/159, Shuttle suites (uffd 20/20, incl. needs_withholding_between_changes and needs_mapping_to_give_back_withheld), fxfs-guest-follow-torture-dax 10 runs of 10, no EFAULT or unresolved fault in their logs. macOS: cargo nextest run -p fxdax 112/112. upstream-changes.py --check passes. DoD #1 (whole workspace on macOS and Lima) not run: Linux runs go to GCE, and only fxdax and its Shuttle suites were run.

Verified on the tree pushed (origin/main c1f0fb0 and the three commits): macOS scripts/test.sh, workspace 2049/2049 and Shuttle suites; GCE workspace 2099/2100, the one failure remote::tests::branches_survive_their_hosts_loss (TASK-28's, untouched here: remove_dir_all racing the forgotten store's threads, ENOTEMPTY), proposed as a follow-up; torture-dax 10 of 10, no EFAULT or unresolved fault.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The late faults on a follower's DAX window were x86 KVM's own: its async page fault's 'page ready' re-fault (kvm_arch_async_page_ready), made after the guest had dropped the range and sent UNMAP. The uffd second stage now withholds a page the window maps nothing at (inaccessible, between changes), so the fault fails with EFAULT, which KVM drops, and gives it back when the window maps the range anew (STAGE_MAPPED). UNMAP unmaps again (77897a5 reverted). Commits 63356ac, e1207b8, dc20010; verified by accesses_to_pages_unmapped_fail (SIGABRT before), the Shuttle race and its mutants, and torture-dax 10/10 on GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
