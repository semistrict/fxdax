---
id: TASK-149.35
title: >-
  A tenant's in-flight data is bounded per connection, not per tenant: 1,024
  connections hold 272 GiB
status: To Do
assignee: []
created_date: '2026-11-06 03:59'
updated_date: '2026-11-07 03:00'
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
- [ ] #1 A test with a tenant's connections each keeping its room full: what the daemon holds of them all, measured
- [ ] #2 A bound per tenant, or the user's decision against one, recorded in OPEN_QUESTIONS.md
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
The user's decision, 2026-11-06: bound memory per tenant too. A tenant's connections share one room, configurable. Recorded in OPEN_QUESTIONS.md under The daemon and the VM process; another agent implements it.
<!-- SECTION:NOTES:END -->
