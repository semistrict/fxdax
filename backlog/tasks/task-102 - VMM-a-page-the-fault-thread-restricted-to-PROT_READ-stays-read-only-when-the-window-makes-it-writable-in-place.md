---
id: TASK-102
title: >-
  VMM: a page the fault thread restricted to PROT_READ stays read-only when the
  window makes it writable in place
status: To Do
assignee: []
created_date: '2026-10-25 00:29'
updated_date: '2026-10-25 00:41'
labels:
  - bug
dependencies: []
priority: high
ordinal: 401000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team, 2026-10-24, fxdax ae54e01, 5 of 26 2 GiB DAX VMs died ('Failure during vcpu run: Bad address'): the stuck page's VMA in the VMM was r--s (no VM_WRITE), uffd_wp=1, registered for write-protect only (uw). UffdSecondStage::resolve restricts a page whose store the window refused to PROT_READ (restrict, rules().refused_store_read_only); window_mapped restores read-write for withheld (PROT_NONE) pages alone, and protect(ReadWrite) only clears UFFDIO_WRITEPROTECT: a page restricted, then made writable in place, keeps its VMA read-only, and every store fails in KVM (hva_to_pfn), no userfault reaching the fault thread. Their traces: one page each, first of a 2 MiB chunk, guest kernel rep_movs, streaks growing before the fatal one. TASK-99's guest race is one way in; this makes any refusal followed by an in-place ReadWrite fatal.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fixed (uffd.rs): PageChanges.restricted, set when a refused store's page is made read-only; protect(ReadWrite) restores PROT_READ|PROT_WRITE on restricted pages, installed or not (a page left out, made writable, faults to the window), and clears it; an install, an unmap and trim's leaving out, which replace the memory, clear it too. Rule protection_lifts_restriction (Rules). Restrictions and lifts logged with offset, made and seen (the sandbox team's ask 2). Shuttle: refusals_are_lifted_by_protections red before ('a store to a page the window maps writable failed'), green after; needs_protections_to_lift_restrictions fails with the rule off. Lima: fxdax Shuttle 34/34, dax:: 17/17. GCE r86 (workspace, suites, pgbench x5) running.
<!-- SECTION:NOTES:END -->
