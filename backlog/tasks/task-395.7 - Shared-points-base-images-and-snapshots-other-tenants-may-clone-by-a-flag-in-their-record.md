---
id: TASK-395.7
title: >-
  Shared points: base images and snapshots other tenants may clone, by a flag in
  their record
status: To Do
assignee: []
created_date: '2026-11-13T08:53:00Z'
labels:
  - bucket
  - collector
  - security
dependencies:
  - TASK-395.3
parent_task_id: TASK-395
priority: medium
ordinal: 779000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Today any tenant can be given another tenant's point to restore, and ForkStore makes a cell of the caller's own whatever the parent's, so inheritance crosses tenants freely. The user (2026-11-13): sharing across real tenants is not a requirement, except a set of base images and snapshots that many tenants clone. Those are marked in the point's record (decided over a reserved tenant ID: no special tenant, any point can be published in place, and tenant 0 already means the daemon's own stores). A record format change.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A point's record says whether it is shared; the trusted control API (and fxdaxctl) makes a point shared at its making or later, and unshares it, which stops new clones only
- [ ] #2 A clone of a point of another tenant's, by restore, ForkStore or any path, is refused unless the point is shared, checked at the read of the point's record the clone already makes; clones within a tenant are unchanged
- [ ] #3 A clone of a shared point writes its reference under the point's cell as any clone does, and the point's cell keeps the point's state while any reference stands, deleted point or not
- [ ] #4 Tests: a tenant clones a shared base; a tenant cannot clone another's unshared point (EACCES); deleting a shared base with live clones in other tenants loses none of their data
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
