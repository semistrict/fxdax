---
id: TASK-48
title: 'extentd: gRPC health check on the control API'
status: To Do
assignee: []
created_date: '2026-10-19 11:13'
labels: []
dependencies: []
priority: medium
ordinal: 147000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Orchestrators and load balancers probe gRPC services with the standard health protocol (grpc.health.v1.Health). The daemon's control API has none, so whether a daemon is serving can only be inferred from a call that does work.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The control API serves grpc.health.v1.Health: SERVING once the daemon accepts stores, NOT_SERVING while it starts or stops
- [ ] #2 Watch streams changes of the status
- [ ] #3 A test checks the status through a daemon's start, serving and shutdown
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
