---
id: TASK-91.16
title: 'VMM: the extentd client thread spends 90-165 us of host kernel time per MAP'
status: In Progress
assignee: []
created_date: '2026-10-23 18:01'
updated_date: '2026-10-24 16:49'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Handed over (2026-10-24): 7 commits on branch review/r7-dax-unverified (on main da7726d8), not to be landed as they are. Their review found three ordering races: a deferred STAGE_UNMAP sent after another thread's fault mapped the same offset; STAGE_PROTECT not undoing a restriction the fault thread made with mprotect; ack IDs reaching the wire out of order once waited unmaps go out outside the MMU lock. They also clash by design with review/r2b-extentd, whose RemoteStage never waits and parks/quarantines replaced pages. Redo on top of r2b once it lands: what still pays is 91fc372f's seal reprotecting pages in place (STAGE_PROTECT, not munmap+mmap: the VM process's munmap cost 45 us an unmap in TLB shootdowns) and 491d3838's VM-process stage counters reported to extentd. GCE measurement on the branch: 'extentd client' sys us/MAP 189 -> 158.

AC1, GCE r66 (2026-10-24, n2-standard-8, pgbench-dax on main's kernel b4, perf record -g -p <firecracker> 40 s from 45 s in, task-clock): the 'extentd client' thread's hottest host-kernel time is TLB shootdowns: smp_call_function_many_cond 3.40% of all the process's samples (87.7 s of CPU in 40 s), 2.49% from native_flush_tlb_multi under unmap_region of mmap(MAP_FIXED) in UffdSecondStage::leave_out, 0.91% kvm_flush_remote_tlbs from the MMU notifier on the same unmaps; then __wake_up_sync_key 1.43%, tdp_mmu_set_spte_atomic 0.65%, tdp_mmu_zap_leafs 0.57%. install's MAP_FIXED, over memory left out and never touched, flushes nothing worth seeing. leave_out runs for the guest's UNMAPs and for trim(), past the stage's budget of runs (half of vm.max_map_count beyond 4096, ~30.7k): the guest's window held 28-30k regions under pgbench, so trim leaves out 1/8 of the runs, each its own mmap, registration and mprotect, and the guest's faults install them again. Hypothesis, to test with vm.max_map_count raised (r68): trim's churn is most of the per-MAP cost and the 8 GiB window's decay.

r68 (GCE us-east1-b, one host, kernel b4, 8 GiB window): the trim hypothesis is false. With vm.max_map_count 65530 the VMM held 53k mappings, with 1048576 113k, and leave_out was 6.6% then 7.4% of the process's samples: it is the guest's UNMAPs, not trim. tps 928/844/849/828/814 at the default, 955/890/884/881/881 raised (+3-8%). Per 60 s run (both alike): ~130k MAPs at ~305 us (40 s of the four clients'), ~36k UNMAPs at ~410 us (14.5 s), ~35k seals, ~90k evacuation MAPs, ~75k regions pruned: the UNMAPs are the pruned regions' window space. Next (r69, kernel b6): freed window space left mapped for the next MAP there to replace, fxfs.dax_unmap_freed=1 for the old behaviour, A/B 3 rounds each.
<!-- SECTION:NOTES:END -->
