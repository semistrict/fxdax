---
id: TASK-79.101
title: >-
  extentd's watchdog cannot tell a VM process from a child that inherited its
  socket, on kernels without pidfd for it
status: To Do
assignee: []
created_date: '2026-10-24 03:39'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 372000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found fixing TASK-79.9 (round 2b). The watchdog that lets go of unanswering VM processes kills and waits by pidfd. A VM process's forked children that keep the socket are not tracked, and on kernels before 6.5 a VM process that exits leaving the socket to a child leaves a pid-reuse race (documented in peer_process's doc).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The watchdog acts on whatever process holds the socket's other end, or refuses a socket shared with children
- [ ] #2 A test forks a child holding the socket and checks the watchdog's outcome
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
