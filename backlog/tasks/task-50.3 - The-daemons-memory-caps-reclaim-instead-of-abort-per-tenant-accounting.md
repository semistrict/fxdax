---
id: TASK-50.3
title: 'The daemon''s memory: caps, reclaim instead of abort, per-tenant accounting'
status: Done
assignee: []
created_date: '2026-10-19 20:10'
updated_date: '2026-10-20 11:55'
labels: []
dependencies: []
parent_task_id: TASK-50
priority: high
ordinal: 152000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
DAX moves each sandbox's file pages from its guest's RAM into extentd's memory, which no VM is charged for, so one tenant can starve a node. Today the daemon's memory is one startup argument (MEMORY_BYTES, 1 GiB by default), it aborts when that runs out, per-tenant arenas have no limits, and it reacts only to its own pressure, not the host's.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A total and a per-tenant cap, each changeable at runtime through the control API
- [x] #2 At a cap the daemon evicts clean pages or refuses new allocations with an error the VM sees, and does not abort; a test fills a tenant past its cap
- [x] #3 Per-tenant memory in /metrics, labelled by a hash of the tenant, not its id
- [x] #4 A control call shrinks the daemon toward a target
- [x] #5 How its memory is charged under cgroup v2 is documented
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Caps in the PMM: a total and per-arena (tenant) limit of 2 MiB chunks, set at runtime; take_page_locked refuses a chunk past either.
2. At a cap, evict the arena's own clean pager-backed pages first (an arena-targeted eviction), then fail the allocation (SHOULD_WAIT for those that can wait, NO_MEMORY otherwise), surfacing to the VM as an errno on its request rather than aborting the daemon; find and remove the abort paths on OOM.
3. Control API: SetMemoryLimits (total, per tenant), ShrinkMemory (evict toward a target), per-tenant memory in /metrics labelled by a hash of the tenant.
4. Test: a tenant filled past its cap (clean pages evicted, then requests refused), the daemon alive; the total lowered at runtime shrinks.
5. Document cgroup v2 charging (the daemon's memfds are shmem, charged to the cgroup that first touched each page).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-20: implemented (uncommitted, pending Linux runs). PMM: per-arena and total chunk caps; an allocation of an arena under pressure (at its cap, the total's, or out of chunks with no other arena's spare) asks extents.rs's relief thread, once per failure, to evict the least-used cached extents' clean pages (own at its cap, any otherwise, 4 MiB a pass); a pass that finds nothing marks the arena exhausted and its allocations fail with NO_MEMORY (ENOMEM to the VM), cleared by the next pass that evicts. Waiters wait on a Pulse (wakes all waiters past a count) instead of an autounsignal event. Watchdog: OomBehavior::Evict (default) never enters OutOfMemory, which aborted the process. Control API: SetMemoryLimit (tenant or total, >= 2 MiB), ShrinkMemory; /metrics: fxdax_extentd_memory_bytes, _limit_bytes, _capacity_bytes, tenant_memory_bytes{tenant=sha256-16}, tenant_memory_limit_bytes. README: cgroup v2 charging. A code review found 8 bugs in the first version (multi-page spin, relief loop never ending at the total cap, sticky exhausted, lost wakeups, registration race, false 'nothing evicted'); all fixed as above. Tests: extents capped_arenas_evict_their_least_used_extents, capped_arenas_refuse_what_they_cannot_evict, kernels_out_of_memory_evict_their_least_used_extents (macOS pass); tests/memory.rs (Linux).

2026-10-20, Linux stress (tests/memory.rs, 8 copies in parallel on a GCE host): a VM read zeros, about 1 in 400 runs. Cause, traced with stage map/unmap and chunk logs: FxExtent::read_unit supplied a unit's pages from its buffer, then each waiting request's range again from the same buffer offsets; evicted between the two, the second supply found the buffer's pages moved out and supplied zero markers / another arena's page. Fixed in fxfs-platform: PageInRange::supplied completes a request its unit's supply covered, without supplying again. 2,400 runs clean after. Also: tests read windows as a vCPU does (tests/common read_window: EFAULT retried), as a load of the test's own SIGSEGVs where the stage swaps a page; ShrinkMemory gives back spare chunks; release_chunk debug-asserts its pages are free.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Per-tenant and total memory limits at runtime (SetMemoryLimit), eviction of the least-used cached extents' clean pages past them or out of memory, else ENOMEM to the VM, never an abort (watchdog OomBehavior::Evict); ShrinkMemory; per-tenant memory in /metrics by name hash; cgroup v2 charging in README. Commit 6f3f5b0, with 021623b (a page-in race eviction exposed). Tests: extents capped_arenas_*, kernels_out_of_memory_*, zircon arena caps, tests/memory.rs (2,400 runs clean under load). Workspace suites pass on macOS and Linux (GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
