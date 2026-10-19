---
id: TASK-26.18.3
title: 'Extent device: MAP takes about 0.85 ms'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-15 08:19'
updated_date: '2026-10-19 06:36'
labels: []
dependencies: []
parent_task_id: TASK-26.18
priority: high
type: bug
ordinal: 57000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The driver now reports time per request type (request_ns). On GCE a MAP takes about 0.85 ms, queued to completed, and creating a small file issues 0.03 of them on the writing thread: about 30 us of a 59 us create, the bulk of create and mkdir-tree being over 2x. Serving requests concurrently on the host did not change it, so the time is in the MAP itself: the daemon, the stage messages to the VM process (mmap, userfaultfd register, mprotect, MADV_POPULATE_WRITE, KVM prefault) or the round trips. crates/fxdax/examples/extentd-mapbench.rs measures it without a VM.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 extentd-mapbench on GCE reports a mean writable MAP of 32 pages under 100 us
- [ ] #2 A GCE bench run shows MAP requests averaging under 100 us in the REQUESTS lines
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extentd-mapbench (crates/fxdax/examples) times MAP writable/read-only, UNMAP, COMMIT without a VM.
2. Run it on GCE with perf: find where the ~0.85 ms goes (daemon, stage ops in the VM process, round trips).
3. Fix the dominant cost; verify with mapbench and a GCE bench run's REQUESTS lines.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Measured 2026-10-19 on GCE n2-standard-8 (extentd-mapbench after TASK-34): writable MAP of 32 pages 160 us mean (267 max), read-only 62 us, unmap 84 us, commit 1.96 ms; the fs bench pgbench run of 10-17 had MAP at ~206 us. Not yet under 100 us: profile the writable MAP next.

2026-10-19 (pushed to main, abb84ad): extentd-mapbench now times every request kind and splits a MAP into the daemon's mapping (Stats::map_ns), the window's changes the VMM process makes (Stats::stage_ns) and the round trip; scripts/example-gce.sh runs it on GCE, under perf with frame pointers. Found: 70% of the daemon's writable MAP was its VMO's pages faulted in from the host (shmem allocation, memcg charge, zeroing), page by page. Not churn (an idle-chunk cache, tried, changed nothing: the arena grows, pages are never reused here), and not per-fault overhead (populating each chunk in one madvise only moved the cost). Fixed by a spare chunk per arena, faulted in on a thread of its own ahead of use (45112ca): daemon mapping 99 -> 40 us, writable MAP 174 -> 116 us mean (n2-standard-8). Left: window 35 us (almost all mmap), round trip ~41 us (a FREE is 34 us: four thread wakeups, VMM caller -> daemon IO thread -> pool thread -> client receiver -> caller). Next: profile what is left of the 40 us; then the wakeups.
<!-- SECTION:NOTES:END -->
