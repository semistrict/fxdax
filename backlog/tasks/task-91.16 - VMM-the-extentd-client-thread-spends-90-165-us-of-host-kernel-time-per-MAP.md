---
id: TASK-91.16
title: 'VMM: the extentd client thread spends 90-165 us of host kernel time per MAP'
status: In Progress
assignee: []
created_date: '2026-10-23 18:01'
updated_date: '2026-10-24 12:29'
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
- [ ] #1 The per-MAP host kernel time attributed with a profile
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
<!-- SECTION:NOTES:END -->
