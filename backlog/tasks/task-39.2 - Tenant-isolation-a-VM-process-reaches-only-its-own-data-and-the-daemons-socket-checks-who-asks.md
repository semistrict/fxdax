---
id: TASK-39.2
title: >-
  Tenant isolation: a VM process reaches only its own data, and the daemon's
  socket checks who asks
status: Done
assignee: []
created_date: '2026-10-17 19:16'
updated_date: '2026-10-18 04:00'
labels: []
dependencies: []
parent_task_id: TASK-39
priority: high
type: feature
ordinal: 112000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every Firecracker process maps the extent daemon's whole memory, which caches every VM's data (crates/fxdax/src/extentd/mod.rs and the README call a host's VMs 'one trust domain'): a guest that escapes into its VMM could read other tenants' data. And anything that reaches the daemon's socket can OPEN, create or FORK any store by its path.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A VM process maps only memory holding its own store's (and its lineage's) extents; a test shows another VM's cached pages are not reachable from it
- [x] #2 The socket admits only callers it authorizes for a store (a capability handed over by the orchestrator, or peer credentials checked against a policy), and OPEN, create and FORK of paths outside it are refused, shown by tests
- [x] #3 doc and README describe the isolation model that replaces 'one trust domain'
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Decision (overnight, user asleep): a daemon is the trust domain. A VM process maps the daemon's kernel's whole physical memory, which caches every store it serves, and keeping per-store memory within one daemon would need per-lineage physical memories in the in-process Zircon, and would give up nothing over separate processes. So: one daemon per tenant (process isolation), and within a daemon, authorization of every path a connection names: the daemon's roots (FXDAX_EXTENTD_ROOTS) bound the stores it serves, and the peer's SO_PEERCRED credentials must be let read (points forked from) or write (stores opened, created, forked to, resized) by the path's permissions, as Unix would let the peer itself; what the daemon makes for a peer is chowned to it. The physical memory goes to a connection only once it opens a store.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented: Peer (SO_PEERCRED), authorize/authorize_in with StoreAccess Read/Write, roots(), Peer::own (chown when the daemon is root). Tests: authorize_tests (permission bits for owner/group/other/root, roots), daemons_with_roots_serve_no_stores_outside_them (socket), tests/isolation.rs vm_processes_reach_their_daemons_memory_alone (two daemon processes; each VM process's memfd holds its own cached extent pages and not the other's). Docs: extentd mod.rs Isolation, README, fxdax-extentd usage.

Superseded 2026-10-18 by TASK-43 (with the user). One daemon per tenant is unacceptable. One extentd serves every tenant, with per-tenant memory arenas, a public base arena, and per-tenant VM sockets from a gRPC Grant.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Isolation model: a daemon is a trust domain (its kernel's memory, which VM processes map, caches every store it serves), so a host runs one per tenant; within a daemon every path a connection names is authorized: inside the daemon's roots (FXDAX_EXTENTD_ROOTS), and readable (snapshot points) or writable (stores opened, created, forked to, resized) by the peer's SO_PEERCRED credentials under the path's permissions; what the daemon makes for a peer is chowned to it. Verified: isolation integration test (two daemons, each VM process's memfd holds its own cached pages and not the other's), authorization unit tests and a socket test with roots; Linux 1997 and macOS 1976 workspace tests; all 13 guest suites pass in Lima.
<!-- SECTION:FINAL_SUMMARY:END -->
