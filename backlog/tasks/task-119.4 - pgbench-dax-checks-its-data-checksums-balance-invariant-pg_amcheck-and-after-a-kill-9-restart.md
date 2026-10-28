---
id: TASK-119.4
title: >-
  pgbench-dax checks its data: checksums, balance invariant, pg_amcheck, and
  after a kill -9 restart
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-28 19:27'
updated_date: '2026-10-28 19:36'
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
initdb --data-checksums; after each run: accounts/tellers/branches balance sums equal history's delta sum, amcheck verify_heapam on the 4 tables (0 corrupt) and bt_index_check(heapallindexed) on the 3 indexes; after the runs: pg_ctl stop -m immediate (a crash as PostgreSQL sees it), start, recover, check again; fast stop; pg_checksums --check. Host expects 'OK checked'. (immediate stop replaces kill -9: orphaned backends of a killed postmaster can block the restart.)
<!-- SECTION:PLAN:END -->
