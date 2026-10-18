---
id: TASK-43
title: >-
  One extentd for every tenant: per-tenant memory arenas, a gRPC control API for
  the Go daemon
status: Done
assignee: []
created_date: '2026-10-18 04:00'
updated_date: '2026-10-18 17:34'
labels: []
dependencies: []
priority: high
type: feature
ordinal: 129000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Replaces TASK-39.2's one-daemon-per-tenant (decided with the user 2026-10-18).

The system's Go daemon keeps launching Firecracker (jailed) and stays. One fxdax-extentd runs beside it, for every tenant on the host.

- **Memory.** The in-process kernel's physical memory is split into arenas, each its own memfd: one per tenant, and one public arena for base images every tenant may read. A store's pages come from its tenant's arena, or the public arena for a public base. A VM process gets its tenant's memfd read-write and the public arena's read-only. Extents are write-once and forks write new ones, so nothing writes a public page once it is sealed.
- **Trust.** The tenant is the security boundary: VMMs of one tenant trust each other, since they share an arena (anything finer would end sharing). Tenants share only read-only public bases.
- **Control.** extentd exposes a gRPC API (tonic over a Unix socket; the .proto lives in this repo for the Go daemon to vendor) for tenants, stores (create, import, fork, delete, resize, place, sync), grants and stats. gRPC passes no fds: VM processes keep the extent protocol.
- **Grants.** Grant(tenant) creates the tenant's VM socket, which the Go daemon bind-mounts into the tenant's jails, and returns its path. A connection's tenant is the socket it came in on. Grants are not persisted: after a restart of extentd, the Go daemon grants again.
- **fxdaxctl.** With extentd running, fxdaxctl is a gRPC client of it. Without one, it works locally (dev, tests, building base images offline), and the cross-process store lock makes it refuse stores extentd has open.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A VM process of one tenant cannot map another tenant's cached pages: its memfds are its tenant's arena and the public arena (read-only), shown by a test with two tenants in one daemon
- [x] #2 Forks of a public base share its pages across tenants: one copy in the public arena, shown by a test
- [x] #3 The Go daemon's operations are a gRPC API over a Unix socket, with the .proto in the repo; a Go-independent client test exercises every RPC
- [x] #4 Grant returns a tenant's socket path; a VM process connecting there opens only that tenant's stores and public bases, shown by tests
- [x] #5 fxdaxctl works through the gRPC API when extentd runs, and locally when it does not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Security model (user, 2026-10-18). Guest VM isolation is the security boundary: a guest reaches only what its DAX window maps. Running every VM as a single tenant, in one arena, is therefore completely secure, though it lacks defence in depth. Per-tenant arenas are the defence in depth: they confine a VMM escape (a guest exploiting the VMM process) to its own tenant's cached pages and the read-only public bases. So the default is one arena that everything uses, as today. Tenants and their arenas are opt-in. Do not go further: no per-VM separation, since sharing within a tenant is essential.

Tenant isolation is very specifically defence in depth only. It is not accepted as a substitute for proper guest isolation, which must hold on its own: nothing may rely on arenas for VM-to-VM security.

Evidence: AC1 tests/tenants.rs tenants_hold_their_own_pages; AC2 public_bases_are_shared_read_only; AC3 tests/control.rs and tests/grants.rs call all 12 RPCs through a tonic client; AC4 tests/grants.rs (tenants_name_stores_in_their_directory_alone, revoked_tenants_take_no_new_connections, tenant_sockets_work_bind_mounted_into_jails); AC5 tests/fxdaxctl.rs (locally and through the daemon). Verified on GCE (2089 tests) and macOS (2041). TASK-43.8 (cross-process flock wait without a thread, low) stays open as a follow-up.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
One extentd serves every tenant: per-tenant memory arenas (43.1, 43.2) as defence in depth, a gRPC control API over a Unix socket with the .proto in the repo (43.3), per-tenant VM sockets from Grant (43.4), fxdaxctl through the API or locally (43.5), waits that hold no thread (43.6), and the default socket as the default tenant (43.7). Every AC has a test, green on GCE and macOS. Open follow-up: TASK-43.8.
<!-- SECTION:FINAL_SUMMARY:END -->
