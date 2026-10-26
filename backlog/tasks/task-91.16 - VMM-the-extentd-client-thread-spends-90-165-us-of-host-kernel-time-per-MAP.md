---
id: TASK-91.16
title: 'VMM: the extentd client thread spends 90-165 us of host kernel time per MAP'
status: In Progress
assignee: []
created_date: '2026-10-23 18:01'
updated_date: '2026-10-26 01:54'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: medium
ordinal: 354000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Round 6: Firecracker's 'extentd client' thread took 5.72 s system for 62,454 MAPs in the DAX load (~92 us each), 13.56 s for 82,277 in a run (~165 us); 0.31 s without DAX. The review session's analysis: an overwrite of a mapped window range pays a STAGE_UNMAP round trip to the VM process, whose trim() scans all 512k pages when over budget (crates/fxdax/src/dax/uffd.rs); its stage counters never reach extentd's stats. To trace with perf record -g of that thread.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The per-MAP host kernel time attributed with a profile
- [ ] #2 Cut, and the VM process's stage counters reported
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Where a MAP's host cost goes today (uffd.rs, server.rs RemoteStage, client.rs): an overwrite is STAGE_UNMAP (leave_out: anonymous mmap over the old memory, the TLB shootdown and KVM zap; UFFDIO_REGISTER; mprotect), STAGE_MAPPED, then STAGE_MAP for the pages MAP_RANGE populates (install: file mmap, UFFDIO_REGISTER, mprotect/UFFDIO_WRITEPROTECT, MADV_POPULATE_WRITE, then KVM_PRE_FAULT_MEMORY), each its own message and recvmsg, the receiver peeking for UNMAPs to join. A seal remaps the open extent's pages, the same physical pages, read-only that way too.
1. Measure: fxfs-guest-pgbench-dax reports each run's VMM threads' user and system CPU (/proc/<vmm>/task), so 'extentd client' system us per MAP is read off every run. Baseline on GCE.
2. extentd: RemoteStage writes the stage operations of one window change (a MAP's UNMAP, MAPPED and MAPs) together, in one write, in the order the kernel made them, flushed at the change's end and before anything waits for or keeps pages until them (IDs taken in order as they are queued, so acknowledgements stay cumulative).
3. VMM: reads whole buffers of messages; an UNMAP followed by MAPs of the same range is made as installs over the old memory (one mmap replacing it), leaving out only what no MAP covers.
4. VMM: where that MAP is of the very pages installed there (a seal), the pages are write-protected in place (UFFDIO_WRITEPROTECT): no unmap, zap or refault.
5. Prefault: measured with and without for pages reprotected and read-only installs.
Proof: the stage's unit tests and Shuttle model for each rule (a test failing with it off), extentd's stage tests, fxfs-guest-checks-dax and pgbench-dax on GCE, and the per-MAP system time against the baseline (target under 50 us).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Handed over (2026-10-24): 7 commits on branch review/r7-dax-unverified (on main da7726d8), not to be landed as they are. Their review found three ordering races: a deferred STAGE_UNMAP sent after another thread's fault mapped the same offset; STAGE_PROTECT not undoing a restriction the fault thread made with mprotect; ack IDs reaching the wire out of order once waited unmaps go out outside the MMU lock. They also clash by design with review/r2b-extentd, whose RemoteStage never waits and parks/quarantines replaced pages. Redo on top of r2b once it lands: what still pays is 91fc372f's seal reprotecting pages in place (STAGE_PROTECT, not munmap+mmap: the VM process's munmap cost 45 us an unmap in TLB shootdowns) and 491d3838's VM-process stage counters reported to extentd. GCE measurement on the branch: 'extentd client' sys us/MAP 189 -> 158.

AC1, GCE r66 (2026-10-24, n2-standard-8, pgbench-dax on main's kernel b4, perf record -g -p <firecracker> 40 s from 45 s in, task-clock): the 'extentd client' thread's hottest host-kernel time is TLB shootdowns: smp_call_function_many_cond 3.40% of all the process's samples (87.7 s of CPU in 40 s), 2.49% from native_flush_tlb_multi under unmap_region of mmap(MAP_FIXED) in UffdSecondStage::leave_out, 0.91% kvm_flush_remote_tlbs from the MMU notifier on the same unmaps; then __wake_up_sync_key 1.43%, tdp_mmu_set_spte_atomic 0.65%, tdp_mmu_zap_leafs 0.57%. install's MAP_FIXED, over memory left out and never touched, flushes nothing worth seeing. leave_out runs for the guest's UNMAPs and for trim(), past the stage's budget of runs (half of vm.max_map_count beyond 4096, ~30.7k): the guest's window held 28-30k regions under pgbench, so trim leaves out 1/8 of the runs, each its own mmap, registration and mprotect, and the guest's faults install them again. Hypothesis, to test with vm.max_map_count raised (r68): trim's churn is most of the per-MAP cost and the 8 GiB window's decay.

r68 (GCE us-east1-b, one host, kernel b4, 8 GiB window): the trim hypothesis is false. With vm.max_map_count 65530 the VMM held 53k mappings, with 1048576 113k, and leave_out was 6.6% then 7.4% of the process's samples: it is the guest's UNMAPs, not trim. tps 928/844/849/828/814 at the default, 955/890/884/881/881 raised (+3-8%). Per 60 s run (both alike): ~130k MAPs at ~305 us (40 s of the four clients'), ~36k UNMAPs at ~410 us (14.5 s), ~35k seals, ~90k evacuation MAPs, ~75k regions pruned: the UNMAPs are the pruned regions' window space. Next (r69, kernel b6): freed window space left mapped for the next MAP there to replace, fxfs.dax_unmap_freed=1 for the old behaviour, A/B 3 rounds each.

r69 (GCE us-east1-b, one host, kernel b6, 8 GiB window, 3 rounds each arm): the guest leaving freed window space mapped (no UNMAP for pruned regions or freed ranges) against unmapping it (fxfs.dax_unmap_freed=1). Lazy: UNMAPs 0 (from ~36k a run), MAPs ~350 us (from ~305), tps 917/817/819/791/813, 982/868/843/803/817, 982/868/832/803/813; unmapping: 917/837/833/798/799, 956/852/816/798/818, 959/848/829/802/793. Equal. And leave_out stayed ~7% of the VMM's samples with no guest UNMAP at all: the host's own window map over a mapped range (map_unsafe SPECIFIC_OVERWRITE) unmaps the old range in zircon's aspace, which sends the VMM STAGE_UNMAP, then STAGE_MAP. So the per-MAP host cost is one TLB shootdown per overwrite of a populated range, which moving the unmap only moves; ~90k of a run's ~130k MAPs are evacuations overwriting ranges' own space. Change not kept (reverted: pins host memory for nothing). Cutting it means fewer remaps (the staging/evacuation design, TASK-91.10/91.4), or a stage operation that replaces a mapping without the separate unmap. The 8 GiB decay (~960 -> ~800) is the same in every arm, map count and unmap policy alike: its cause is elsewhere.

Design read (2026-10-24): a seal (extents.rs seal_mapped) maps the sealed extent's VMO over each window mapping of the open one (make_read_only: map_unsafe SPECIFIC_OVERWRITE, the aspace's unmap -> STAGE_UNMAP -> leave_out, one shootdown), supplies the open VMO's pages to it (the same physical pages), then populates (STAGE_MAP of those pages read-only). Reprotecting in place (91fc372f on review/r7-dax-unverified) saves the unmap's zap, the free and the refaults, but a write->read downgrade (UFFDIO_WRITEPROTECT or mprotect) flushes the host's TLBs and KVM's too: about half the cost per seal, not the shootdown. Removing it needs the protection deferred and batched (one flush for many pages), applied before the sealed pages are shared (cached.sealing). The r7 branch's ordering races (a deferred unmap after another thread's fault mapped the offset, PROTECT not undoing a fault's mprotect, acks out of order) are the risk; Shuttle models exist there (shuttle_tests.rs faults_race_deferred_unmaps).

Sandbox team round 6 (TASK-109, b746104), perf of the VMM's extentd client thread over a 60 s DAX pgbench run: 21.1 s system for 120,091 MAPs (~175 us each), 3.9 s without DAX. mmap(MAP_FIXED) 38% inclusive (unmap_region 25%, flush_tlb_mm_range 11.7%, smp_call_function_many_cond 13.9%, kvm_mmu_notifier_invalidate_range_start 10.8%), KVM_PRE_FAULT_MEMORY 12.3%, recvmsg 13%. Syscalls: 705,266 recvmsg (165,437 without DAX), 351,240 mmap, 638,499 ioctl, 307,721 mprotect, 143,522 sendmsg: ~2.9 mmaps and 5.3 ioctls per MAP. External-interrupt exits 472k against 295k (shootdown IPIs to vCPUs). Their ask: under 50 us system per MAP, by in-place reprotection (TASK-92.50's unlanded branches) and batching window changes so one shootdown and prefault cover many pages.

Baseline r114b (main + the VMM thread sampler, 117d2516e; GCE us-east4-a, pgbench-dax, 1 VM, load + 5 runs): tps 1069/1022/989/934/890, load 16.8 s. Over the VM: extentd client sys 102.69 s (user 9.25) for 568,782 window MAPs (414,245 requests), 193,730 UNMAPs, 156,197 seals: ~180 us system per MAP. vCPUs sys 58.0 + 38.8 s.

r116 (23759aa8a: corked changes, remap in the VMM, prefault unchanged; same setup as r114b): tps 1082/1001/989/918/915, load 17.2 s. extentd client sys 98.47 s for 698,618 MAPs, 243,117 UNMAPs, 188,996 seals: 141 us per MAP (baseline 180), 87 us per window change counting MAPs, UNMAPs and seals (baseline 112): -22%. Far from the 50 us target. r117 drops the prefault of pages reprotected in place; r119 profiles the extentd client thread (perf -g, perf trace -s) with the change, to see what's left.

r115b (c06c3cdbc, GCE, Linux): fxdax + zircon workspace tests 2,637 passed (and the other suites), the new wire tests (corked sends, unmaps taking the maps within them) and remaps_end_as_steps_do among them. The remapped seal-and-free race failed only its reach check: no schedule reached an access failing (EFAULT, retried), as a seal made at once leaves its pages in place; required events now exclude it (6b22a2a46). Ported 491d3838 (VM processes report stage counts, STAGE_COUNTS renumbered 27, FREE_MANY took 26): ae046fc9a. r115c reruns fxdax at HEAD.

r117 (63eaee74f, r116 without prefaulting pages reprotected in place): tps 1127/1046/1000/961/938, load 16.4 s; extentd client sys 97.75 s for 728,907 MAPs, 256,846 UNMAPs, 198,188 seals: 83 us per window change (r116 87, baseline 112), 134 us per MAP. Adopted (pages reprotected in place are mapped for the guest as it touches them).
<!-- SECTION:NOTES:END -->
