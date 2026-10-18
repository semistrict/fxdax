---
id: TASK-43.4
title: 'Grants: per-tenant VM sockets returned by Grant, bind-mountable into jails'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-18 04:00'
updated_date: '2026-10-18 06:55'
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
- [x] #1 A VM process on a tenant's socket opens its tenant's stores and public bases, and is refused another tenant's (test)
- [x] #2 The socket works bind-mounted into another mount namespace (Lima test)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Kernel: removing an arena that still holds chunks retires it; it goes once its last chunk goes back.
2. extents::evict_arena drops the cache entries and transfer allocator of an arena.
3. Tenant gets a directory; its connections name stores in it alone (store_key canonical, no `..`), and control requests over a tenant socket are refused (EPERM). Tenant Drop evicts its arena and removes it.
4. extentd/grants.rs: Grant(tenant, dir, uid, gid) binds <tenants dir>/<tenant>.sock (0660, chowned), served by serve_tenant on a thread; again with the same dir returns it, another dir EEXIST; Revoke unlinks and shuts the listener down.
5. proto: Grant, Revoke. Binary: FXDAX_EXTENTD_TENANTS, default tenants/ beside the control socket.
6. Tests: tests/grants.rs (directory checks, revoke, a jail: sudo unshare --mount, bind mount, tenants dir hidden by tmpfs, setpriv to the jail user), extentd::tests::tenants_arenas_go_with_them, arena_unittest removed_arenas_go_once_their_chunks_do.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
From TASK-43.3: tenants have no RPCs of their own. Grant(tenant, uid) makes the tenant (its arena) if new and its VM socket; Revoke closes the socket and removes the tenant once its connections are gone, evicting its arena's cached extents (pmm::remove_arena needs the arena's pages free).

Tests run on GCE, not Lima (user rule, 2026-10-18): scripts/test-gce.sh run gained FXDAX_TEST_ARGS (nextest arguments) and FXDAX_GUEST_TESTS=0. AC 2 ran on GCE (as root, via sudo unshare), not Lima.

Validation: GCE full workspace 2050 passed; macOS 2012 passed; the new tests 200/200 under --stress-count on GCE; upstream-changes --check OK. Guest e2e not rerun: no x86-64 guest kernel at hand (the daemon binary only gained FXDAX_EXTENTD_TENANTS; VM connections of no tenant authorize as before). scripts/license-headers.py fails on crates/zircon/src/sync.rs, from cc02679 (another session), not this task.

The default VM socket, of no tenant, keeps TASK-39.2's peer-credential and FXDAX_EXTENTD_ROOTS checks (removing them would delete their tests); tenant sockets use the directory check instead.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The control API grants tenants VM sockets of their own: Grant(tenant, dir, uid, gid) returns <tenants dir>/<tenant>.sock, bind-mountable into jails, whose connections name stores in the tenant's directory alone (EACCES otherwise, symlinks and .. resolved) and may not make control requests; Revoke removes the socket, and the tenant, its cached extents and its arena go once its VM processes do (the kernel retires an arena until its last chunk goes back). Verified on GCE and macOS: tests/grants.rs including a jailed VM process in its own mount namespace through the bind-mounted socket, tenants_arenas_go_with_them, removed_arenas_go_once_their_chunks_do.
<!-- SECTION:FINAL_SUMMARY:END -->
