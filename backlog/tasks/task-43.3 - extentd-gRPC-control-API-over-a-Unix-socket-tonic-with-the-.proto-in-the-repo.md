---
id: TASK-43.3
title: >-
  extentd gRPC control API over a Unix socket (tonic), with the .proto in the
  repo
status: To Do
assignee: []
created_date: '2026-10-18 04:00'
labels: []
dependencies: []
parent_task_id: TASK-43
priority: high
type: feature
ordinal: 132000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Service for the Go daemon: tenants (create, delete), stores (create, import an image, fork, delete, resize, place, sync, stat), grants, stats. Errors as gRPC statuses carrying errnos. Replaces the custom socket's control requests (FORK_PATH, DELETE_PATH, RESIZE, SYNC_PATH, STATS) for callers outside VM processes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Every RPC exercised by a client test in Rust (no Go)
- [ ] #2 The .proto documented and versioned (fxdax.extentd.v1)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
