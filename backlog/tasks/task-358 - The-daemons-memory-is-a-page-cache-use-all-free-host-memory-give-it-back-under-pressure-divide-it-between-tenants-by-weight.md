---
id: TASK-358
title: >-
  The daemon's memory is a page cache: use all free host memory, give it back
  under pressure, divide it between tenants by weight
status: To Do
assignee: []
created_date: '2026-11-11 14:59'
labels: []
dependencies: []
priority: high
ordinal: 742000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
User, 2026-11-11: the extent daemon's memory must work like Linux's page cache. Today it is a fixed pool set at start (fxdax-extentd's MEMORY_BYTES, else physmem::DEFAULT_MEMORY_BYTES = 1 GiB), with per-tenant caps (TASK-50.3, control API SetMemoryLimit). On ubu1 (docs/measurements/ubu1-loop-baseline-2026-11-11) random reads over a working set past 1 GiB fell to 43-66 IOPS while the host had free RAM, where the baseline's host page cache took all of it. Under DAX the daemon's memory is a guest's only file cache, and fxdax is the only thing on its hosts: no memory may sit idle.

Linux's page cache has no size: it takes whatever is free, kswapd reclaims it from low watermarks (MemAvailable), and cgroups protect shares (memory.low/min) and weigh reclaim. Do the same:
- Grow into free host memory, no fixed cap; keep a headroom for VMs' own memory and the host.
- Shrink under pressure: watch the host's (or its cgroup's) available memory / PSI, and evict least-recently-used extents first, before VMs or the host are squeezed.
- Divide between tenants by a weight per tenant over the gRPC control API (e.g. the memory the tenant's sandboxes are allocated), as cgroup's proportional protection does; a tenant's share is protected under pressure, unused shares are lent out.
- Replace the fixed MEMORY_BYTES and per-tenant byte limits; no migration of the old settings (AGENTS.md).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The daemon starts with no fixed memory size and caches extents until host available memory falls to a headroom; benchmark: random reads over a working set of 4x the old 1 GiB stay in memory on a host with that much free
- [ ] #2 Under host memory pressure (another process allocating), the daemon gives memory back within seconds, evicting least-recently-used extents; a test shows it
- [ ] #3 The gRPC control API sets a weight per tenant; under pressure each tenant keeps at least its weighted share while it uses it; a test shows two tenants' shares
- [ ] #4 MEMORY_BYTES and per-tenant byte limits removed, docs updated
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
