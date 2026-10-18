---
id: TASK-43
title: >-
  One extentd for every tenant: per-tenant memory arenas, a gRPC control API for
  the Go daemon
status: To Do
assignee: []
created_date: '2026-10-18 04:00'
updated_date: '2026-10-18 04:02'
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
- [ ] #1 A VM process of one tenant cannot map another tenant's cached pages: its memfds are its tenant's arena and the public arena (read-only), shown by a test with two tenants in one daemon
- [ ] #2 Forks of a public base share its pages across tenants: one copy in the public arena, shown by a test
- [ ] #3 The Go daemon's operations are a gRPC API over a Unix socket, with the .proto in the repo; a Go-independent client test exercises every RPC
- [ ] #4 Grant returns a tenant's socket path; a VM process connecting there opens only that tenant's stores and public bases, shown by tests
- [ ] #5 fxdaxctl works through the gRPC API when extentd runs, and locally when it does not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Security model (user, 2026-10-18). Guest VM isolation is the security boundary: a guest reaches only what its DAX window maps. Running every VM as a single tenant, in one arena, is therefore completely secure, though it lacks defence in depth. Per-tenant arenas are the defence in depth: they confine a VMM escape (a guest exploiting the VMM process) to its own tenant's cached pages and the read-only public bases. So the default is one arena that everything uses, as today. Tenants and their arenas are opt-in. Do not go further: no per-VM separation, since sharing within a tenant is essential.

Tenant isolation is very specifically defence in depth only. It is not accepted as a substitute for proper guest isolation, which must hold on its own: nothing may rely on arenas for VM-to-VM security.
<!-- SECTION:NOTES:END -->
