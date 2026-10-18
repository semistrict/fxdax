---
id: TASK-43.4
title: 'Grants: per-tenant VM sockets returned by Grant, bind-mountable into jails'
status: To Do
assignee: []
created_date: '2026-10-18 04:00'
labels: []
dependencies:
  - TASK-43.2
  - TASK-43.3
parent_task_id: TASK-43
priority: high
type: feature
ordinal: 133000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Grant(tenant) creates the tenant's VM socket (or returns it) and its path, owned by a uid the caller names (the jailer's), for the Go daemon to bind-mount into the tenant's jails. A connection's tenant is the socket it came in on; it opens that tenant's stores and public bases only. Revoke(tenant) closes it. Replaces TASK-39.2's path-permission and FXDAX_EXTENTD_ROOTS checks for VM connections.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A VM process on a tenant's socket opens its tenant's stores and public bases, and is refused another tenant's (test)
- [ ] #2 The socket works bind-mounted into another mount namespace (Lima test)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
