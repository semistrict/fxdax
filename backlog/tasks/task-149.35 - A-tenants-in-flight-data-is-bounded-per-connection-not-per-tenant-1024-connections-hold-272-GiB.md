---
id: TASK-149.35
title: >-
  A tenant's in-flight data is bounded per connection, not per tenant: 1,024
  connections hold 272 GiB
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-06 03:59'
updated_date: '2026-11-07 03:55'
labels:
  - bug
  - extentd
  - security
dependencies: []
parent_task_id: TASK-149
priority: high
ordinal: 645000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-149.17. The protocol bounds what the daemon holds of a VM process's requests per connection (Peer::room: MAX_STREAM_BYTES 256 MiB, QUEUED_ROOM 16 MiB, its window's mappings), and a tenant has MAX_CONNECTIONS (1024) connections: one tenant's VM processes, within the protocol, may have the daemon hold some 272 GiB of stream data at once, every tenant's daemon with it. The hostile test checks each connection's room, three connections at a time; no test opens hundreds keeping their rooms full. A design question for the user: a bound per tenant (shared by its connections, a connection past it let go or made to wait), or fewer connections, or a smaller room.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test with a tenant's connections each keeping its room full: what the daemon holds of them all, measured
- [x] #2 A bound per tenant, or the user's decision against one, recorded in OPEN_QUESTIONS.md
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Measure: a pinned test (hostile.rs a_tenants_in_flight_data_is_bounded) of one tenant's connections flooding reads whose replies go unread, a small per-tenant room set, measuring the tenant's held and the heap; a second tenant flooding, unaffected.
2. Bound: memory::InFlight per tenant, DEFAULT_TENANT_ROOM 8 GiB, a request's stream data and queued replies counted in it; a request past it waits at admission (Connection::within_bounds), holding no thread.
3. Configurable: control SetTenantRoom / memory::set_room, per tenant or daemon, as the memory limits.
4. A mutant without the gate, killed by the pinned test; the hostile proptest's floods reach it.
5. Docker runs.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-07: Measured (page_ins/room pinned test): 8 flooding connections of one tenant held 2,132 MiB without a bound (each up to its own 272 MiB room), the protocol's 1,024 connections x 272 MiB ~272 GiB a tenant. Bound: memory::InFlight, a room shared by a tenant's connections (DEFAULT_TENANT_ROOM 8 GiB), counting each request's stream data and the replies the daemon queues for a VM process; Connection::within_bounds takes from it before reading a request's payload, waiting (holding no thread) until what the tenant holds leaves room, or going on if it holds nothing (so a request as large as the room is served). AsyncSender::counting counts its queued bytes in the tenant's room, released as the socket takes them or the sender drops. Not let-go or EAGAIN: the stream carries a guest's I/O, which must be served; a healthy tenant moves data through transfer areas (256 KiB x 8 a connection), uncounted. Default 8 GiB: a host's density case held 16 MiB of a tenant's stream data; 8 GiB is ~30 VMs each with a full connection room, reached only writing 64 MiB extents through the stream back to back. Configurable: control SetTenantRoom, memory::set_room, per tenant (alive now or granted later) or daemon-wide, floor MAX_PAYLOAD. Pinned test: 8 connections held 64 MiB of a 64 MiB room (2,132 without the gate); a second tenant flooding was served throughout. Blind spot: the hostile harness opened few connections a tenant; still 1-3. Class: a_tenants_in_flight_data_is_bounded opens 8, each flooding. Mutant a-tenant-shares-no-room (the gate replaced by an unconditional add), killed by it. Docker: pinned test red (2,132>128) and green (64); hostile 500 cases pass; 12 of 12 mutants killed; fxdax, extent_store, physmem 707 of 711, the 4 the container's (git, sudo). Decision in OPEN_QUESTIONS.md.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A room shared by a tenant's connections (memory::InFlight, DEFAULT_TENANT_ROOM 8 GiB), counting their requests' stream data and queued replies; a request past it waits at admission. Configurable per tenant or daemon (control SetTenantRoom). Found a tenant could hold ~272 GiB (1,024 connections x 272 MiB); pinned test a_tenants_in_flight_data_is_bounded (8 connections held 2,132 MiB without the bound, 64 with it, a second tenant unaffected), mutant a-tenant-shares-no-room. Verified in Docker; decision in OPEN_QUESTIONS.md. GCE/macOS DoD not run here.
<!-- SECTION:FINAL_SUMMARY:END -->
