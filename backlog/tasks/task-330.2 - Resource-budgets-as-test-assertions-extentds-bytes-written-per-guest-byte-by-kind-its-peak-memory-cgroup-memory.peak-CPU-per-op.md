---
id: TASK-330.2
title: >-
  Resource budgets as test assertions: extentd's bytes written per guest byte by
  kind, its peak memory (cgroup memory.peak), CPU per op
status: To Do
assignee: []
created_date: '2026-11-10 14:00'
labels:
  - testing
dependencies: []
parent_task_id: TASK-330
priority: high
ordinal: 731000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-310 added per-kind written-bytes counters to extentd's stats; TASK-317 added cgroup peak memory to test-e2e.sh (FXDAX_EXTENTD_MEMORY_MAX). Turn them into assertions with budgets in the e2e and bench cases. Example: write amplification at most 1.3x the guest data plus framing; peak anonymous memory at most the configured budgets plus a fixed slack. A tuning change that trades one metric for another then fails here, not at a customer.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 TASK-310's double write and TASK-317's 6 GB growth, each put back, fail these assertions
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
