---
id: TASK-403
title: >-
  Sim deleted_forking: a store's log found diverged though the branch's last
  commit is the log's (seed 100732, all rules on)
status: To Do
assignee: []
created_date: '2026-11-13 18:54'
labels:
  - bug
  - bucket
  - sim
dependencies: []
priority: medium
ordinal: 787000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-395.10's seed search for deletions_need_fences (rules all on): SIM_PROFILE=deleted_forking SIM_SEED=100732 cargo nextest run --cargo-profile sim -p extent_store --run-ignored only -E 'test(=sim::one)' fails with "branch 2's log diverged, though the branch's last commit, None, is the log's", on t3/395-per-cell-collector at bb07304a6 as on t3/395-references (where seed 15364 fails the same way: "... last commit, Some(10000000002), is the log's"). Outside the 200 seeds the profile's soak runs and the 0..20,000 the rule searches run, so no test failed. Writer side (wal::rebase / Wal::heads), not the collector's references. Find the cause; add the seed to the profile's test_holds found list once fixed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause is found and fixed; seeds 100732 and 15364 of deleted_forking hold, pinned in its test_holds list
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
