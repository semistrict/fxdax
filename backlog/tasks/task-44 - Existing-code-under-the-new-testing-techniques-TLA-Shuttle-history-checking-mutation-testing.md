---
id: TASK-44
title: >-
  Existing code under the new testing techniques: TLA+, Shuttle, history
  checking, mutation testing
status: To Do
assignee: []
created_date: '2026-10-18 04:05'
labels: []
dependencies: []
priority: high
ordinal: 129000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-42 (followers) brought testing techniques the project hadn't used: TLA+ specs model-checked with mutation checks (TASK-42.8, make check-tla), Shuttle for controlled schedules of real Rust code, history checking against committed states, and mutation testing (rule switches, cargo-mutants). The existing code's concurrency has had bugs these would have caught: the 6.18 DAX SIGBUS (dce4f14: a write(2) consumed a page's relocation mark; on 6.12 only a silent WARN_ON_ONCE), the uploader's panic in close_now (an extent freed between commits). Each subtask applies them where the concurrency and the bug history are; the user agreed on 2026-10-18. Every check gets a mutation check: a check that never fails proves nothing.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The subtasks are done
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
