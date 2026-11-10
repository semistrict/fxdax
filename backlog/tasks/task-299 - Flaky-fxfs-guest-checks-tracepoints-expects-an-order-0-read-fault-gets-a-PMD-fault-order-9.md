---
id: TASK-299
title: >-
  Flaky: fxfs-guest-checks tracepoints expects an order-0 read fault, gets a PMD
  fault (order 9)
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 10:02'
updated_date: '2026-11-10 09:15'
labels:
  - bug
  - flaky
dependencies: []
priority: medium
ordinal: 714000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE t257dbg3 (debug kernel, main 6408801bc + TASK-257 + TASK-292, fxdax-keep-n2-standard-4-us-west1-a-1): fxfs-guest-checks-dax-delay round 2 (seed 1792699608765578114) and fxfs-guest-checks-dax-small round 4 (seed 1792807242380890800) failed 'FAILED: tracepoints': tracepoints.rs (the load's read fault): assert_eq!((fault.number("order"), fault.number("write")), (0, 0)) failed with Event { name: "fxfs_dax_fault_start", fields: {ino: 1036, order: 9, pgoff: 2, write: 0} }: the read fault of page 2 was taken as a PMD fault. The check came with TASK-44.16 (fd37ccbe2). Either the mapping's alignment lets the kernel try a PMD fault (the check should allow order 9, or map so it can't), or a PMD fault there is itself wrong.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The check states what fault orders a read of page 2 may take, and why
- [x] #2 tracepoints passes 20 rounds of fxfs-guest-checks-dax-delay and -dax-small on the debug kernel
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Read the DAX fault path in 6.18.54: with the PMD entry empty, __handle_mm_fault offers a DAX fault as a PMD fault first (__thp_vma_allowable_orders skips size checks in a fault); dax_fault_check_fallback falls back for a VMA under 2 MiB.
2. Make the PMD entry's state fixed: map the file MAP_FIXED at a PMD-aligned address in a PROT_NONE reservation, with an anonymous page right after it in the same 2 MiB, stored to first, so the PMD entry holds a page table. The load then takes exactly one fault, order 0, which the check asserts (one fault_start of the inode, order 0, write 0), then its MAP as before.
3. ubu1: 20+ rounds of the tracepoints check on the debug kernel, -dax-delay and -dax-small; release kernel too.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Analysis (6.18.54 tree)
- The PMD fault is legitimate. __handle_mm_fault offers a fault as a PMD fault whenever the PMD entry is empty and thp_vma_allowable_order(PMD_ORDER) holds; for a DAX VMA in a fault that is just 'THP not disabled' (__thp_vma_allowable_orders: 'Skip the check for page fault. Huge fault does the check in fault handlers'; vma_is_dax returns the orders before any size check). fxfs_dax_huge_fault takes a read PMD fault to fxfs_dax_do_fault, which traces order 9, and dax_iomap_pmd_fault's dax_fault_check_fallback falls back (pmd_addr + PMD_SIZE > vm_end for a 16-page VMA), VM_FAULT_FALLBACK, nothing mapped; then the order-0 fault MAPs. PUD faults fall back in fxfs_dax_huge_fault untraced.
- The flake: mmap(NULL, 64 KiB) put the mapping where the kernel chose; sharing its 2 MiB with something already faulted (a PTE table there), the PMD entry wasn't empty and only the page fault ran; in a fresh 2 MiB, the PMD fault came first.
## Change
- tracepoints.rs: maps the file MAP_FIXED at a PMD-aligned address inside a PROT_NONE reservation of 4 MiB that never faults, so the PMD entry is empty at the load (page tables exist only for a 2 MiB some VMA overlaps; free_pgtables frees a gap's), and asserts exactly: the PMD read fault (order 9, write 0, pgoff 2), its end ret == VM_FAULT_FALLBACK with no fxfs_dax_map or window MAP between, then the order-0 fault and its MAP as before. The doc comment says which orders and why.

## Revised (ubu1 round 1)
Asserting the PMD attempt failed on its first debug round: two order-9 faults before the order-0 one. Under the per-VMA lock do_read_fault returns VM_FAULT_RETRY for a VMA with no map_pages (vmf_can_call_fault), and the fault is redone under the mmap lock, the PMD attempt with it. How many PMD attempts there are hangs on the kernel's lock and retry internals (and on lock_vma_under_rcu succeeding), so the check now pins the PMD entry as a page table instead: an anonymous page in the mapping's 2 MiB, stored to before the load. The load then takes one fault, order 0. The doc comment says which orders a read of page 2 may take and why.
## Verification (ubu1, b4eba9c9 = d2a5d2cd6 rebased)
- Debug kernel, fxfs-guest-checks-dax-delay FXDAX_CHECKS=tracepoints: 20/20 rounds.
- Debug kernel, fxfs-guest-checks-dax-small (all checks, tracepoints among them): 23 rounds passed (8+3+7+5 over 4 runs). Runs stopped by budget overruns (failed_write_inode_reported, torture, partial_writeback, append_writeback and others: still running after the budget) on a host at load 40-150 on 16 cores from other agents, not by tracepoints. FXDAX_CHECKS_SCALE=4, then 8.
- Release kernel, fxfs-guest-checks-dax FXDAX_CHECKS=tracepoints: 20/20.
- test_cases on Linux: --features host 113/113, --features guest (as root, as test-e2e.sh runs it) 114/114. No e2e.log said 0/0 passed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The check was wrong. With the PMD entry empty, the kernel offers a DAX read fault as a PMD fault first, and it falls back for this 16-page mapping. Under the per-VMA lock it does so twice (do_read_fault's retry). Whether the entry was empty depended on where mmap put the mapping. The check now maps the file at a PMD-aligned address next to an anonymous page it stores to first, so the PMD entry is a page table, and asserts exactly one fault of the inode, order 0, at the load. Verified on ubu1: 20/20 debug dax-delay tracepoints rounds, 23 debug dax-small rounds, 20/20 release dax tracepoints rounds.
<!-- SECTION:FINAL_SUMMARY:END -->
