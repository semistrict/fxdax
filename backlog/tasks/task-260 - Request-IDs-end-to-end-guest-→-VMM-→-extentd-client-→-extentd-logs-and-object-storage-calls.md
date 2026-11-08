---
id: TASK-260
title: >-
  Request IDs end to end: guest → VMM → extentd client → extentd logs and
  object-storage calls
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-08 14:04'
updated_date: '2026-11-08 14:51'
labels:
  - logging
  - extentd
  - vmm
dependencies: []
priority: medium
ordinal: 692000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A slow or failed guest I/O cannot be followed across layers today: the guest, the VMM, the daemon and object storage each log without anything tying a record to the guest request it serves, so correlating a daemon warning or a slow GET with the guest operation behind it means guessing by time. The guest driver already numbers every request (virtio_extent_req.unique, doc-4, "for tracing"), which nothing reads. Carry it through so the daemon logs, extent_remote's object-storage logs and a future OpenTelemetry agent name the guest request. Constraints: old guests and VMMs keep working (an absent ID logs as none); the ID stays cheap.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Every daemon log record emitted while serving a VM process's call for a guest request carries that request's ID (request_id), including requests deferred across threads (faults, PRIVATE, opens) and object-storage work done on the remotes' runtime for it
- [ ] #2 extent_remote logs each object-storage request (kind, key, time, outcome) and those made for a guest request carry its ID
- [ ] #3 An old VMM (ids without the guest request) and an old daemon both keep working with the new side; records of calls without an ID say none
- [ ] #4 One well-named function exposes the current request for an OpenTelemetry agent, documented in the task notes, with which ID is used and why
- [ ] #5 Tests: the wire id round trip, propagation across polls and threads, and a daemon log record carrying the guest request of a VMM call
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. fxdax::request: the current guest request (RequestId { guest, store }), thread-local, carried across polls (InRequest) and threads (within). 2. VMM: serve each virtio request with its header unique current (guest_request). 3. Daemon wire v4: a REQUEST flag (bit 31) on any request, its guest number the first 8 payload bytes; codec decodes/strips it (guest_request). 4. Daemon: serve each call with the request current, with the connection store path, deferred futures included. 5. Carry it to the remotes runtime (Fetcher::read, read-ahead) and the pager. 6. Log: request_id and store on records; extent_remote logs each object-storage request at debug. 7. Tests: codec round trip, propagation, daemon log records, object-storage record.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Which ID and why: the guest driver per-device request counter, virtio_extent_req.unique (doc-4, for tracing), which every request already carries: no guest or virtio change, no cost. In the daemon it is paired with the connection store path (RequestId.store), which tells one VM device from another, as every device numbers its own requests.

Wire (user decision 2026-11-08: no deployments, follow AGENTS.md): daemon wire VERSION 4. Any request may carry wire::REQUEST (flag bit 31), its guest number the first REQUEST_BYTES (8, little-endian, non-zero) of its payload, before the kind payload; codec::guest_request reads and strips it, and decode refuses one that says 0 or is short. VM processes of version 3 are refused (EPROTONOSUPPORT), as any other version. The Header layout is unchanged (what no version moves). An earlier draft marked the top bit of Header.id and kept version 3; dropped.

For OpenTelemetry: fxdax::request::current() -> Option<RequestId { guest: NonZeroU64, store: Option<Arc<str>> }>, the request the work on this thread is for: in the VMM as it serves the request, in the daemon as it serves a call, and as object storage and the pager work for it (carried by request::within to the remotes runtime, read-ahead and the pager). The header alone does not hold the number (it is in the payload), so a hook of a Header can only say whether one is there; current() holds it wherever the work runs.

Paths: crates/fxdax/src/request.rs; extentd/wire.rs, codec.rs, client.rs, server.rs; remote.rs; cache.rs; crates/extent_remote/src/store.rs (made(): each object-storage request logged at debug with kind, object, ms, error); vendor/firecracker extent/worker.rs (guest_request(unique, ...)), fork branch land-reqid 81f82bb27.
<!-- SECTION:NOTES:END -->
