---
id: TASK-395.7
title: >-
  Shared points: base images and snapshots other tenants may clone, by a flag in
  their record
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-13T08:53:00Z'
updated_date: '2026-11-13 09:29'
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_remote: Record.shared (Flags.shared, only on points, else Corrupt); FORMAT 7 -> 8, 7's records refused (Error::Format); Error::NotShared(point) and Error::NotPoint(branch).
2. branch::share(bucket, point, shared): CAS of the point's record; NoBranch, Deleted, NotPoint refused.
3. branch::finish_point_fork: the read of the point's record the fork already makes (now made also when the child's tenant differs from the point's, whatever forks_check_points says) refuses a child of another tenant's cell unless shared (NotShared). Every clone of a point (restore via fork_from, ForkStore from a point copy, fxdaxctl, kept images) goes through Remote::forked -> fork_point -> finish_point_fork.
4. fxdax: NotShared -> EACCES, NotPoint -> EINVAL; host_store::share / ExtentStore::share(path, shared) from the store's description (branch, bucket); daemon: SharePoint rpc, ForkStore.shared (point made, then shared; EINVAL without point); EACCES not the point store's fault (no let-go, no retry); fxdaxctl share/unshare/point --shared; go/ regenerated.
5. Tests: record flags/format, branch::share and refusal (extent_remote); a point forks into another tenant's cell only while shared, and its clone keeps its data after the point is deleted and collected (fxdax remote.rs); fxdaxctl local share/unshare; Linux grants: restores of another tenant's point refused until shared, ForkStore of shared points into other tenants.
6. AC #3 (references) left to TASK-395.3. Writable cross-tenant forks (ForkStore of a tenant-0 public base into a tenant's directory) left as they are: follow-up task.
<!-- SECTION:PLAN:END -->
