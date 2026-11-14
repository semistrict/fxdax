---
id: TASK-395.9
title: >-
  Forks of branches with writers into another tenant's cell: only from the
  common tenant's
status: Done
assignee:
  - '@claude'
created_date: '2026-11-13 09:42'
updated_date: '2026-11-14 12:52'
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
- [x] #1 A fork of a store with a writer (ForkStore, fxdaxctl fork/point, host_store::fork) to a cell of another tenant's than the store's branch is refused with EACCES, making nothing, unless the store's tenant is 0
- [x] #2 Tenant-0 stores, public bases included, still fork into any granted tenant; tenants.rs public_bases_are_shared_read_only and grants.rs tenants_stores_are_their_own_cells pass unchanged
- [x] #3 Tests: a non-zero tenant's writable store forked into another tenant's directory is refused with EACCES, leaving nothing in the bucket; a tenant-0 store forks into two tenants as before
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. host_store::may_fork_into(parent, remote, cell): Ok for a point's store (TASK-395.7 checks its record), a branch of the same tenant, or a parent of tenant 0 (NO_TENANT); EACCES otherwise.
2. Called by host_store::fork before anything is made, and by ExtentStore::new_child before the child's ID is taken from the bucket, so fork, fork_quiescent, point (ForkStore, fxdaxctl, restores of writable stores) refuse first.
3. Tests: remote.rs unit test (macOS): tenant-1 writable store refused into tenant 2 and 0 by fork/fork_quiescent/point, bucket listing unchanged, nothing at the path; tenant-0 store forks into tenants 1 and 2. grants.rs (Linux): acme's writable base forked/pointed into globex's directory refused EACCES, nothing made.
4. Docs: proto ForkStore comment (go regenerated), OPEN_QUESTIONS.md decision, stores.rs fork doc.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
e3cc9c016: host_store::may_fork_into(parent, remote, cell): Ok for a point's store (TASK-395.7 checks its record), the same tenant, or a parent of tenant 0 (NO_TENANT, now documented as the common tenant); EACCES otherwise. Called in ExtentStore::new_child (before the bucket gives the child's ID: fork, fork_quiescent, point, so ForkStore, fxdaxctl fork/point, restores of a store that is no point) and in host_store::fork (fork_to, point_to, images). Docs: proto (Go regenerated), stores.rs fork, OPEN_QUESTIONS.md decision. No Go daemon test covers forks; none changed.
Tests: remote.rs stores_with_writers_fork_into_other_tenants_cells_from_the_common_tenant_alone (tenant 1 refused into 2 and 0 by fork, fork_quiescent, point, fork_to, point_to; nothing at the path; bucket listing unchanged; forks into its own; tenant-0 store forks into 1 and 2, each reading its bytes). Mutant: without new_child's check, it fails (a refused fork allocated an objectid object). grants.rs tenants_stores_with_writers_fork_into_no_other_tenants_directory (ForkStore fork, point, shared point of acme's into globex's directory: PermissionDenied/EACCES, nothing made; tenant-0 base into both).
Verified: macOS clippy gate clean; targeted nextest 47/47; Linux zig cross-check clean; scripts/test.sh: workspace 2357/2357, sim 61/61, Shuttle 37+10+6+33+10+18, mutants 0 broken, go ok; upstream-changes.py --check ok; ubu1 run t3959a (a25bb1658, workspace only, FXDAX_GUEST_TESTS=0: no guest case forks a non-zero tenant's writer store): 2629/2629, sim 61/61; grants.rs and tenants.rs public_bases_are_shared_read_only, tenants_stores_are_their_own_cells pass unchanged.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Forks of stores with writers into another tenant's cell are made only from tenant 0's stores (the user's decision); from any other tenant they fail with EACCES before anything is made, in the bucket too (host_store::may_fork_into, in ExtentStore::new_child and host_store::fork). Public bases and shared points unchanged. Verified by a new unit test (bucket unchanged, mutant caught) and a grants.rs daemon test; macOS scripts/test.sh and ubu1 workspace run t3959a pass.
<!-- SECTION:FINAL_SUMMARY:END -->
