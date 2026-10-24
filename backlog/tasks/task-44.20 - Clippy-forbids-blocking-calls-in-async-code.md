---
id: TASK-44.20
title: Clippy forbids blocking calls in async code
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: high
ordinal: 399000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-98: the image device waited on a condvar inside an async task (through block_in_place), and fxdaxctl import deadlocked for hours. Clippy's disallowed-methods, configured in clippy.toml, names the blocking calls (Condvar::wait, block_in_place, thread::sleep, blocking store calls) so that using them needs an explicit allow with a reason.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 clippy.toml disallows the blocking calls, and the workspace's clippy run is clean
- [ ] #2 Each remaining use is allowed explicitly, with why it can't deadlock
- [ ] #3 TASK-98's old block_in_place wait, put back, fails clippy
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
