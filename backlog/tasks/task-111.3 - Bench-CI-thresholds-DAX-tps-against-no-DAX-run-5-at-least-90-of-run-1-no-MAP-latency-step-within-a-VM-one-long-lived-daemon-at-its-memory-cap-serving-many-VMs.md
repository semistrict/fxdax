---
id: TASK-111.3
title: >-
  Bench CI thresholds: DAX tps against no DAX, run 5 at least 90% of run 1, no
  MAP latency step within a VM, one long-lived daemon at its memory cap serving
  many VMs
status: To Do
assignee: []
created_date: '2026-10-28 01:25'
labels: []
dependencies: []
parent_task_id: TASK-111
priority: medium
ordinal: 417000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sandbox team asks for pass/fail checks on the pgbench comparison so regressions fail in our CI, not on their box (TASK-111). DAX >= no DAX is the goal and a known failure today.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The CI job fails when run 5 tps is under 90% of run 1
- [ ] #2 The CI job fails on a step change in MAP latency within one VM
- [ ] #3 The CI job runs many VMs in a row on one daemon whose memory is full from the start (TASK-108.1)
- [ ] #4 DAX tps >= no DAX is reported, as a known failure until it passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
