---
id: TASK-149.34
title: A connection's page-ins in flight are bounded by nothing it keeps within
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 03:59'
updated_date: '2026-11-07 23:54'
labels:
  - bug
  - extentd
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 644000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found looking around TASK-149.17's memory finding. A request's own copies are a piece at a time now, and its stream data within the connection's room; but a fault, or a read, of a cold extent pages it in, a 1 MiB part at a time with read-ahead (cache.rs read_pages, extent_frame::PART), holding a part's buffer while it reads, and a fault waiting for the pager holds no worker (TASK-92.20), so a VM process's faults on many cold extents at once page in as many parts at once as it has faults in flight (MAX_REQUESTS, 4096), uncounted. One cold 64 MiB READ held about 31 MiB past the room in what_requests_hold_is_within_the_room, which reads its extent once first for that. Unmeasured: what a storm of faults on distinct cold extents holds.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A storm of faults, and of reads, on distinct cold extents, as many as a connection keeps in flight, measured by the counted heap of extentd/tests/hostile.rs
- [x] #2 What it holds is within a bound per connection, or the bound is the daemon's own, stated, and the hostile test checks it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Measure: a pinned test (extentd/tests/hostile.rs page_ins_in_flight_are_bounded) of 256 faults, then 256 reads, on distinct cold extents of a MiB, a point's restored on another host, bucket reads held (GatedReads), counting the tenant's memory and the heap.
2. Bound in extentd's server: a connection's faults and PRIVATEs resolve PAGE_INS (64) at once, a tokio Semaphore each takes before the window resolves it, holding nothing while it waits; reads are bounded already by the connection's 8 workers.
3. The hostile proptest floods page-ins: the given point's 12 extents of a MiB, evicted cold, mapped and faulted at once, by restores of the point always and by mallory's acts.
4. A mutant without the bound, killed by the pinned test; Docker runs.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-07: Found: a connection's faults page in as many parts at once as it keeps faults in flight (MAX_REQUESTS, 4,096), as a fault waiting for the pager holds no worker (TASK-92.20): each part's frames of the tenant's memory are allocated before its read and held until its fetch from the bucket answers. 256 faults on cold extents of a MiB, their fetches held: 258 MiB of the tenant's memory, 123 MiB of the heap as they answered (8 fetches in flight, the rest waiting with their frames). Reads of the same, 8 MiB: bounded by the connection's 8 workers already. Bound: Connection::page_ins, a tokio Semaphore of PAGE_INS (64) that a fault and a PRIVATE take before the window resolves them, released once resolved: those past it wait as futures, holding nothing; 64 x (1 + PARTS_AHEAD) x 1 MiB = 192 MiB at most. Now 66 MiB and 39 MiB. Why 64: a guest faults as many pages at once as it has threads faulting, its vCPUs and on x86 KVM's async page faults' workers, 64 a vCPU queued (ASYNC_PF_PER_VCPU); 64 parts at once from object storage at 30-100 ms is hundreds of MiB a second for one VM; faults on pages in memory release at once. Not counted against the room in bytes: page-ins are the cache's, shared by every store reading an extent, not a connection's, and their frames are the tenant's memory, which its limit and relief already account; a count per connection bounds what one VM process keeps paging in. EAGAIN isn't possible: a guest's fault must be resolved. Blind spot: the hostile harness's extents were warm, its store's own, and its faults few; it now floods page-ins of a point's cold extents (evicted first, a restore of the point flooding as it opens), 1,300 extents in 500 cases. Mutant page-ins-of-every-fault-at-once (the semaphore at MAX_PERMITS), killed by page_ins_in_flight_are_bounded. Docker: the pinned test red without the bound (258 > 192) and green with it; the hostile test 3 x 48, 96 and 500 cases pass; 11 of 11 mutants killed; fxdax, extent_store and physmem 706 of 710, the 4 the container's (git, sudo).

Finalized in the backlog sweep: its commits are on origin/main. DoD #1 by Main CI 42423368353 at 878aa8848 (which holds this work) green, macos (workspace) and test (workspace) among it; scripts/upstream-changes.py --check passes on main (2026-11-08 sweep).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A connection's faults and PRIVATEs page in 64 at once (server::PAGE_INS, a semaphore a connection), where each fault paged in its parts with nothing bounding how many a connection kept in flight. The hostile harness floods page-ins (Act::PageIns) and pins page_ins_in_flight_are_bounded; mutant page-ins-of-every-fault-at-once is killed by it. Landed on main (2b7ec7ec6, 6714f7f73); main's CI green.
<!-- SECTION:FINAL_SUMMARY:END -->
