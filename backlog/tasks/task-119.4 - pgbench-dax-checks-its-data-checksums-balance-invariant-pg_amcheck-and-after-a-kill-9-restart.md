---
id: TASK-119.4
title: >-
  pgbench-dax checks its data: checksums, balance invariant, pg_amcheck, and
  after a kill -9 restart
status: To Do
assignee: []
created_date: '2026-10-28 19:27'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: high
ordinal: 440000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
pgbench-dax runs 5 x 60 s and checks only a row count after the load. Postgres wrong-data bugs (TASK-59) were found by the sandbox team, not us.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 initdb uses --data-checksums
- [ ] #2 After the runs, history deltas sum to the accounts, tellers and branches balances
- [ ] #3 pg_amcheck passes on all indexes, before and after a kill -9 of the postmaster and restart
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
