---
id: TASK-108.1
title: >-
  extentd at its memory cap: VMs initialise 2.6-3.8x slower and run at half the
  tps, worse with each VM
status: To Do
assignee: []
created_date: '2026-10-25 19:06'
updated_date: '2026-10-25 23:39'
labels:
  - bug
dependencies: []
parent_task_id: TASK-108
priority: high
ordinal: 408000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox report round 5: with one 8 GiB daemon serving ten pgbench VMs one after another, the VMs started once it reached its cap took 53-77 s to initialise (19-21 s below it) and ran 407-547 tps (991-1,080 below), each slower than the last. A busy host always sits at the cap (TASK-103 keeps the cache warm up to it by decision), so these are the numbers production sees. Leads unmeasured: evict_least_used sorts every cached extent (up to 65,536) on each relief and evicts whole extents; an extent's recency is stamped only on cache lookups, not as windows fault its pages, so a base extent every fork maps may look oldest and be evicted while in use, its pages fetched again (25 ms from the bucket).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A VM started on a daemon at its memory cap initialises and runs pgbench within 10% of one started on a fresh daemon (FXDAX_PGBENCH_VMS=10 on GCE)
- [ ] #2 Where the time at the cap went is recorded, with counters or stacks
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Reading (unmeasured): at the cap each relief pass frees RELIEF_BYTES, 4 MiB (pmm/relief.rs), and allocations wait for it; extents::evict_least_used, the pass's evictor, gathers and sorts every cached extent of the arena (up to 65,536) under the cache's lock, then evicts whole extents, all their pages, in order of their last cache lookup, not of their pages' use. So paging in N MiB at the cap costs N/4 passes, each slower as entries accumulate with each VM's extents: matches each VM being slower than the last. r102 (FXDAX_PGBENCH_VMS=10, 8 GiB) reproduces.

GCE r102 (tree 417c88b12, FXDAX_PGBENCH_VMS=10, 8 GiB daemon, n2-standard-8, 5 x 60 s runs per VM): no cliff, a steady decay. Init (pgbench -i -s 50) 19.6, 19.7, 21.2, 22.0, 22.9, 23.9, 24.2, 25.7, 26.0, 26.2 s; first-run tps 936, 939, 922, 908, 890, 840, 854, 820, 778, 827. extentd reached Critical pressure during VM 1 and evicted ([EVICT] pager:563M ...); page-ins flat, ~130 MB per VM, so re-fetching evicted pages isn't what grows. Every request kind's mean latency grows ~20% over the ten VMs: MAP 85.7 -> 105 us (~600k per VM, the largest total), WRITE 198 -> 229-244, FREE 55 -> 69, READ 309 -> 369, UNMAP flat. A shared cost growing with what the daemon accumulated (stores, cached extents, window mappings: cf. TASK-92.50, MAP cost growing with the VMO's mappings), not eviction itself. The sandbox team's 2.6-3.8x is not reproduced here (differences: their n2-standard-16, Debian postgres:17 base, one run per VM, RustFS behind netem). Next: profile extentd (perf) during VM 1 and VM 10.

GCE r108 (95af36bac, same setup): the same decay, init 19.0 -> 27.0 s, first-run tps 957 -> 805; within each VM tps falls run to run too (957 -> 797 over its five runs, VM 1 included), a second effect growing with the store. Its profile captured nothing (pgrep -f matched another process); r110 profiles extentd (pgrep -x) for 20 s in VM 1's and VM 10's first run.

r110 (b1a73f2b9, 10 VMs): the decay again, VM 1 -> 10 init 18.9 -> 27.3 s, first run 976 -> 835 tps, fifth run 783 -> 711. Its profiles caught the harness's idle extentd (pgrep -x picks the first; pgbench's daemon runs from /tmp/fxdax-pgbench.*); r111 picks it by its arguments and profiles VMs 1, 5 and 10, with perf diff.
<!-- SECTION:NOTES:END -->
