---
id: TASK-108.1
title: >-
  extentd at its memory cap: VMs initialise 2.6-3.8x slower and run at half the
  tps, worse with each VM
status: To Do
assignee: []
created_date: '2026-10-25 19:06'
updated_date: '2026-10-26 03:35'
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

r111 (b1a73f2b9, 10 VMs, perf of pgbench's own extentd for 20 s in the first run of VMs 1, 5, 10): extentd's CPU doesn't grow, it falls: 9.56G, 9.42G, 8.65G task-clock in 20 s (~0.48, 0.47, 0.43 CPUs), the same shape of profile (receive, serve_in_turn, write_with_barriers 20-22%, commit_with 16%, ExtentStore::map 13-15%, Window::map 11-12%), while its threads park more: epoll_wait 9.9% -> 12.3% -> 19.1% inclusive, finish_task_switch 7.4% -> 12.8%. So the ~20% per-request latency growth and the tps decay aren't extentd's CPU: it waits on the VMs more. Next: the VMM's threads per VM (the pgbench sampler now records them, 1d7e41579) and the host kernel (shmem/memfd of the 8 GiB arenas, KVM) across 10 VMs.

Sandbox at 139e82b: across 12 2 GiB DAX VMs on one 8 GiB extentd, tps 944-1,041 with no downward trend (11% fall over 15 at b746104), extentd's memory 3.4-7.6 GB, never at its cap: neither the at-cap slowdown nor a drift reproduced there (n2-standard-16, 64 GB). Our harness still decays (r111: VM 1 954 -> VM 10 821 first-run tps) on n2-standard-8 (32 GB): host memory, not extentd's CPU (r111), is the suspect; r118 records it system-wide and off-CPU.

r118 (117d2516e, 10 VMs, VMM thread sampler): the VMM's extentd client thread grows across VMs: VM 1 105.1 s sys (user 13.1), VM 10 147.6 s sys (user 18.5), +40%, while extentd's own CPU falls (r111) and tps falls 939 -> 781 (first runs). The window's changes cost the VMM more in later VMs on one host: per-MAP normalisation and the host-wide/off-CPU profiles (r118-perf) next.

r120 (cac2afe77, 5 VMs on one 8 GiB extentd, n2-standard-8 32 GB): the VMM's host time per window change grows 84.6 -> 94.5 -> 103.9 -> 112.1 -> 123.4 us (+46%), tps first runs 1094 -> 961, as the host's free memory falls 12.0 -> 4.8 -> 4.4 -> 3.3 -> 2.4 GB. extentd's cache (RssShmem) reaches its 8 GiB cap from VM 2 and stays; its anonymous memory grows ~0.7 GB a VM (0.6 -> 3.8 GB): the harness's bucket is in extentd's memory (memory:///, FXDAX_REMOTE_LATENCY_MS=25), every VM's uploads kept. The VMM's own page tables and RSS stay flat. So our decay is the harness's in-memory bucket starving a 32 GB host, which costs the host kernel's memory operations (the VMM's window changes) more as it goes; the sandbox team's real bucket on 64 GB hosts shows none at 139e82b. To confirm: the same 5 VMs on n2-standard-16 (FXDAX_GCE_MACHINE, bfea65a73).
<!-- SECTION:NOTES:END -->
