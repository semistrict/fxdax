---
id: TASK-94
title: 'extentd: per-tenant disk limits: each tenant''s host disk counted, and capped'
status: To Do
assignee: []
created_date: '2026-10-24 12:29'
updated_date: '2026-11-06 06:51'
labels: []
dependencies: []
references:
  - TASK-50.3
  - TASK-79.89
  - TASK-43.4
priority: medium
type: feature
ordinal: 378000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Memory has per-tenant limits (TASK-50.3: SetMemoryLimit, eviction past a tenant's cap, ENOMEM otherwise). Disk has none. A store's capacity bounds only what its guest sees: the bytes its branch's extents hold, inherited ones included. A fork of a 10 GiB base starts 10 GiB "used" while storing nothing new. So no cap bounds what a tenant puts on the host's disk, and one tenant can fill it for every other.

Filesystem quotas on a tenant's directory (TASK-43.4/43.7) cannot do this: a fork's directory is only a handle, and its new data is written into the base store's directory (extent_store, Lineage), outside the tenant's.

Sharing makes counting a decision, to be made with the user before implementing: charge data shared between branches to the branch that wrote it (a fork pays for what it writes, a public base's owner for the base), or split it between the tenants holding it. Space a freed extent leaves in a segment until cleaning reclaims it (TASK-79.89) counts too.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each tenant's host disk bytes are counted under one documented rule for shared data, and reported per tenant by the control API and /metrics (labelled by a hash of the tenant, as memory is)
- [ ] #2 A per-tenant disk limit, set and changed at runtime through the control API, and a total one: past it, the tenant's VMs' writes fail with ENOSPC, the daemon does not abort, and other tenants' writes go on
- [ ] #3 Tests: a tenant filling its limit through forks of a shared base is refused while another tenant writes; freeing data, once cleaning reclaims it, lets the tenant write again
- [ ] #4 README documents how a tenant's disk is counted and limited, and why directory quotas do not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-06 tidy: two premises above changed since. TASK-165.7 (67740ed72): a store's directory holds its branch alone, and a fork's child is a store of its own, its new data in its own directory, no longer written into the base's (no Lineage). TASK-165.8 (a31f82a98): local cleaning and its dead space (TASK-79.89) are gone; a store keeps on local disk only what its uploader hasn't had acknowledged (bounded by the lag bound, 1 GiB by default) and its open segment, and acknowledged data goes to the host's part cache, shared by every tenant and bounded by its capacity. So what a tenant can take of a host's disk is its stores' unacknowledged segments, about the lag bound each, and its use of the shared cache: the counting rule for shared data (this task's open decision) now applies to the cache. AC #3's 'once cleaning reclaims it' reads 'once it leaves local disk'.
<!-- SECTION:NOTES:END -->
