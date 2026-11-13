---
id: TASK-395.9
title: >-
  Forks of branches with writers into another tenant's cell: refused, or through
  shared points
status: To Do
assignee: []
created_date: '2026-11-13 09:42'
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
TASK-395.7 refuses a fork of a point into a cell of another tenant's unless the point is shared, checked at the read of the point's record the fork makes. A fork of a store with a writer is not checked: ForkStore of any store into a granted tenant's directory makes a cell of that tenant's whatever the store's tenant (crates/fxdax/src/extentd/stores.rs fork, tenant = the directory's grant), and TASK-395's decision says cross-tenant inheritance is needed only for shared points. Existing behaviour relies on it: a tenant-0 public base (MakePublic) forked into tenants' directories (crates/fxdax/tests/tenants.rs public_bases_are_shared_read_only; crates/fxdax/tests/grants.rs tenants_stores_are_their_own_cells forks a store of no tenant's into acme's directory). Refusing it changes how public bases reach tenants (a shared point of the base first) and those Linux tests; left out of TASK-395.7 for that reason (OPEN_QUESTIONS.md, Decisions).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A fork of a store with a writer (ForkStore, fxdaxctl fork/point, host_store::fork) to a cell of another tenant's than the store's branch is refused with EACCES, making nothing
- [ ] #2 Public bases reach other tenants through a shared point of them; MakePublic and its tests (tenants.rs, grants.rs, Go daemon tests) follow
- [ ] #3 Tests: a writable store's fork into another tenant's directory is refused; a public base's shared point forks into two tenants, their pages shared read-only as before
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
