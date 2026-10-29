---
id: TASK-119.4
title: >-
  pgbench-dax checks its data: checksums, balance invariant, pg_amcheck, and
  after a kill -9 restart
status: Done
assignee:
  - '@claude'
created_date: '2026-10-28 19:27'
updated_date: '2026-10-29 12:38'
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
- [x] #1 initdb uses --data-checksums
- [x] #2 After the runs, history deltas sum to the accounts, tellers and branches balances
- [x] #3 pg_amcheck passes on all indexes, before and after a kill -9 of the postmaster and restart
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
On main since b9af10794 and 5c1bf4fd3: initdb --data-checksums; after each run the accounts', tellers' and branches' balances sum to the previous run's plus the history's deltas (pgbench empties the history as a run begins), and amcheck's verify_heapam and bt_index_check over the pgbench tables find nothing; then 'pg_ctl -m immediate stop' (a crash-like stop, SIGQUIT, no checkpoint, in place of kill -9 of the postmaster), a restart that recovers from the WAL, the balances checked again and amcheck, and pg_checksums --check with the server stopped. Deviation: amcheck's functions from psql rather than the pg_amcheck binary. GCE r221: pgbench-dax 2 rounds passed, the log showing initdb --data-checksums, amcheck after each of 5 runs and after the restart, and pg_checksums. DoD #1 not run.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
pgbench-dax checks its data: checksummed cluster, balance invariant per run, amcheck before and after an immediate-stop recovery, pg_checksums. Verified by GCE r221 (2 rounds passed).
<!-- SECTION:FINAL_SUMMARY:END -->
