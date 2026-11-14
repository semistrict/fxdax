---
id: TASK-395.9
title: >-
  Forks of branches with writers into another tenant's cell: only from the
  common tenant's
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-13 09:42'
updated_date: '2026-11-14 12:04'
labels:
  - bucket
  - security
dependencies: []
parent_task_id: TASK-395
priority: medium
ordinal: 781000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-395.7 refuses a fork of a point into a cell of another tenant's unless the point is shared, checked at the read of the point's record the fork makes. A fork of a store with a writer was not checked: ForkStore of any store into a granted tenant's directory made a cell of that tenant's whatever the store's tenant (crates/fxdax/src/extentd/stores.rs fork, tenant = the directory's grant).

The user's decision (2026-11-14): this is needed only for the common tenant, tenant 0, the daemon's own stores (stores made alone, public bases). A fork of a store with a writer into a cell of another tenant's is made only when the store's tenant is 0; from any other tenant it is refused (EACCES), making nothing. Public bases (MakePublic, tenant 0) keep reaching tenants as they do, with no shared point first; shared points (TASK-395.7) stay as they are.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A fork of a store with a writer (ForkStore, fxdaxctl fork/point, host_store::fork) to a cell of another tenant's than the store's branch is refused with EACCES, making nothing, unless the store's tenant is 0
- [ ] #2 Tenant-0 stores, public bases included, still fork into any granted tenant; tenants.rs public_bases_are_shared_read_only and grants.rs tenants_stores_are_their_own_cells pass unchanged
- [ ] #3 Tests: a non-zero tenant's writable store forked into another tenant's directory is refused with EACCES, leaving nothing in the bucket; a tenant-0 store forks into two tenants as before
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. host_store::may_fork_into(parent, remote, cell): Ok for a point's store (TASK-395.7 checks its record), a branch of the same tenant, or a parent of tenant 0 (NO_TENANT); EACCES otherwise.
2. Called by host_store::fork before anything is made, and by ExtentStore::new_child before the child's ID is taken from the bucket, so fork, fork_quiescent, point (ForkStore, fxdaxctl, restores of writable stores) refuse first.
3. Tests: remote.rs unit test (macOS): tenant-1 writable store refused into tenant 2 and 0 by fork/fork_quiescent/point, bucket listing unchanged, nothing at the path; tenant-0 store forks into tenants 1 and 2. grants.rs (Linux): acme's writable base forked/pointed into globex's directory refused EACCES, nothing made.
4. Docs: proto ForkStore comment (go regenerated), OPEN_QUESTIONS.md decision, stores.rs fork doc.
<!-- SECTION:PLAN:END -->
