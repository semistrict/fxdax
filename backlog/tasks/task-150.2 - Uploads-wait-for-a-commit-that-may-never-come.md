---
id: TASK-150.2
title: Uploads wait for a commit that may never come
status: To Do
assignee: []
created_date: '2026-11-01 16:49'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 517000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed. crates/extent_store/src/store.rs:919-921, :1341, :1671, :1756; segments.rs:682; crates/fxdax/src/remote.rs:1125-1130: a segment make_room closes stays Phase::Framing until the next commit flushes, so take_closed will not release it. COMMIT_DURABLE, SyncStore and a live fork upload wait block until another commit arrives. Whether Fxfs serialises its commits so that a guest cannot wait so is not established.
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
