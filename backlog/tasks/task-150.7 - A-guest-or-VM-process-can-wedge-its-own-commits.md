---
id: TASK-150.7
title: A guest or VM process can wedge its own commits
status: To Do
assignee: []
created_date: '2026-11-01 16:51'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: low
type: bug
ordinal: 522000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed. crates/fxdax/src/extents.rs:1081-1089: the open-extent path does not check alignment, so a guest WRITE leaves open.len unaligned, and every later commit fails EINVAL. crates/fxdax/src/extentd/server.rs:1041: WRITE, MAP writable, FORK without FORK_QUIESCENT seals the extent while open holds it, and later commits fail EPERM. Harm is limited to the caller store.
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
