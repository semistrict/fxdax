---
id: TASK-164.4
title: 'Tests: guest/sim''s host copies on write, and specs/dax-cow'
status: Done
assignee: []
created_date: '2026-11-04 07:58'
updated_date: '2026-11-04 16:35'
labels: []
dependencies: []
parent_task_id: TASK-164
priority: high
type: task
ordinal: 614000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sim's model of the host's copy-on-write, and a TLA+ spec with mutations (doc-13).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Scenarios: rebind against stores, writeback against stores, truncate of dirty pages
- [ ] #2 Each mutation and mutant doc-13 lists is caught; zloop 3,000 seeds clean
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
guest/sim models the host's copy-on-write, with scenarios and a mutant per rule (writeback against stores, rebind, truncate of dirty pages, a crash between bytes and commit, reclaim during writeback, fsync during writeback), its oracle checking the file synced alone; 3,000 seeds per push clean, 16,750 more copy-on-write seeds clean, six dax.c bugs found and fixed; specs/dax-cow (TLA+) holds over 2.0M states, its mutations each breaking their invariant. Verified: macOS scripts/test.sh (workspace 2,089/2,089, Shuttle, scripts); GitHub CI, every job (run 41859737932: Linux workspace, Shuttle incl. the uffd private-memory models, guest-sim's 3,000 seeds and mutants); GCE r546 copy-on-write off in the jailer under the production seccomp filter (5 suites, refusal gate clean), r547 copy-on-write on (crash-dax, pgbench-dax-cow, pgbench-dax-cow-forked, bigmap-dax-cow pass).
<!-- SECTION:FINAL_SUMMARY:END -->
