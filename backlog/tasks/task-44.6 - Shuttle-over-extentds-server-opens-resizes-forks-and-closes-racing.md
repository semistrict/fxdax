---
id: TASK-44.6
title: 'Shuttle over extentd''s server: opens, resizes, forks and closes racing'
status: To Do
assignee: []
created_date: '2026-10-18 04:06'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: medium
ordinal: 135000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
crates/fxdax/src/extentd/server.rs serves stores to connections: one opener per store (open_paths, EBUSY), RESIZE through the connection holding the store or directly (a shrink needs the guest's agreement, TASK-37), FORK of an open store, closes. About 29 lock uses and 4 threads, with tests that drive it from real sockets one schedule at a time.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 In-process connections drive the server's request handling under Shuttle: opens, resizes (growing, shrinking with the guest agreeing or not), forks and closes, racing
- [ ] #2 Invariants: a store has one opener; a resize ends with the store's capacity, the device's configuration and the guest's view agreeing; no fork misses a commit before it; a store closed is reopenable
- [ ] #3 Mutation checks: each rule (the opener check, resizing through the holder, the guest's agreement to a shrink) turned off makes a Shuttle test fail
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
