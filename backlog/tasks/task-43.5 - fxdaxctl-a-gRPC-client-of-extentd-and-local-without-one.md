---
id: TASK-43.5
title: 'fxdaxctl: a gRPC client of extentd, and local without one'
status: To Do
assignee: []
created_date: '2026-10-18 04:00'
labels: []
dependencies:
  - TASK-43.3
parent_task_id: TASK-43
type: feature
ordinal: 134000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
With extentd's gRPC socket named (FXDAX_EXTENTD_GRPC), every fxdaxctl command goes through the API; without, commands act on stores locally, which the cross-process store lock keeps off stores extentd has open.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fxdaxctl's tests pass both ways
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
