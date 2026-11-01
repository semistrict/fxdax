---
id: TASK-150.4
title: OPEN_FORK_FROM lets a VM process take any branch by its ID
status: To Do
assignee: []
created_date: '2026-11-01 16:50'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 519000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed. crates/fxdax/src/extentd/server.rs:1176-1179, :1256-1260; stores.rs:561-572, :664: the paths are authorized, args[1], the branch, is not. A compromised VM process of tenant A, knowing B branch ID, adopts it: its child reads B data, B writer is fenced and its commits fail EROFS. What the check should be may need a decision.
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
