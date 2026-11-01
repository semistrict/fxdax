---
id: TASK-150.5
title: Concurrent write OPENs leak a path hold
status: To Do
assignee: []
created_date: '2026-11-01 16:50'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: medium
type: bug
ordinal: 520000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed. crates/fxdax/src/extentd/server.rs:1208-1231, with open_stores.rs:124-163: the store.get() check and the later set are not atomic, and failed sets are ignored. Two concurrent OPENs on one connection leave a path held, EBUSY until the daemon restarts, and one store without its flush-behind task.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test fails on the code as it stands, or shows the finding false or unreachable, which closes this as not a bug
- [ ] #2 Fixed, the test red again with the fix taken out
- [ ] #3 The notes name the blind spot, the test of its class, and what a search around it found
- [ ] #4 Passes on Linux in cargo nextest run --workspace
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
