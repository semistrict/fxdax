---
id: TASK-44.17
title: >-
  Coccinelle rules for the guest's C: unlocked check-then-act on region and
  range fields, unchecked allocation fallbacks
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: low
ordinal: 396000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Some of the guest's bugs have a shape a semantic patch matches: a field read under one lock and acted on under another (TASK-99), a fallback path taken only when an allocation fails (the keep path). Coccinelle, run by the guest checks, flags new instances as they are written.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Semantic patches for those shapes run in scripts/check-guest.sh
- [ ] #2 They flag TASK-99's code, put back, and pass on main
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
