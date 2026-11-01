---
id: TASK-150.3
title: A window size voids the connection memory bound
status: To Do
assignee: []
created_date: '2026-11-01 16:50'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 518000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed. crates/fxdax/src/extentd/server.rs:1071, :1718-1721, :803; crates/fxdax/src/dax.rs:472-476: Peer::room() grows with a window size the VM process chooses, uncapped. A WINDOW of 1<<60 then 64 MiB WRITEs faster than served hold up to MAX_REQUESTS x 64 MiB in the daemon, which serves every tenant.
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
