---
id: TASK-395
title: >-
  The collector scales to thousands of tenants, millions of branches and 100M+
  objects: a pass per cell, conservative
status: To Do
assignee: []
created_date: '2026-11-13T08:52:00Z'
labels:
  - bucket
  - collector
dependencies: []
references:
  - crates/extent_remote/src/collector.rs
  - specs/bucket/Bucket.tla
priority: high
ordinal: 772000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxdax-gc is one process doing one pass over the whole bucket under one lease (`gc/lease`), and its costs grow with the whole bucket, not with what changed. Measured from the code (2026-11-13): a pass lists the bucket 5+ times (once whole, then `tenants/` until no new record, again after recovery), holds every listed object in a map (~300-400 B each: 30-40 GB at 100M objects), re-recovers every live branch every pass (2 LIST + 2 GET + uncovered segments each, in lockstep rounds of 16), reads moves footers and compacts one object at a time, and deletes one object per request. Nothing is partitioned: references cross cells (every fork makes a new cell) and tenants (restores of another tenant's point), so no part of the bucket can be collected without the rest.

Target: thousands of tenants, millions of VMs (branches), a single tenant possibly holding a large share of them.

**Decided with the user (2026-11-13):**
- The collector may be **conservative**: reclaiming late, and wasting some space for a time, is fine. That is what makes sharding cheap.
- **The unit of collection is the cell**, not the tenant, so that one large tenant is collected as independently as many small ones. Everything a cell's collection decides by is under the cell's prefix: its branches, their moves, its collector lease, and **references** from forks in other cells.
- **A fork keeps its parent's state at the fork point** (the extents it names, the objects and moves ways holding them, what recovering it reads) in the parent's cell for as long as the fork lives, ignoring what the fork frees of it since: the conservative step. A fork's child adds only what is inherited, so a child's descendants are covered by the child's reference, kept while the child's cell has references of its own.
- **Cross-tenant inheritance is not a requirement**, except shared base images and snapshots cloned by many tenants: those are points marked **shared** in their record, set only through the trusted control API; a clone of a point of another tenant is refused unless it is shared. No tenant is special to the collector.

The old whole-bucket pass goes when the per-cell one lands: no two collectors side by side. Formats change outright (no migrations).
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
