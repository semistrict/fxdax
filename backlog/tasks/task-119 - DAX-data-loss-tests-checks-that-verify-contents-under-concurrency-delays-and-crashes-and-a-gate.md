---
id: TASK-119
title: >-
  DAX data-loss tests: checks that verify contents under concurrency, delays and
  crashes, and a gate
status: To Do
assignee: []
created_date: '2026-10-28 19:26'
labels:
  - testing
dependencies: []
priority: high
ordinal: 436000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A survey on 2026-10-28 found: 7 of 28 guest DAX correctness bugs were first found by pgbench, benchmarks or the sandbox team, 6 with no test that would have caught them. The concurrent DAX checks (seal_race, log_writes, faults_*, append_while_mapped) check no contents, or only after a quiet final rewrite; random_ops, the only model-checked generator, is one thread, one file, seed 1; no test enables fxfs.dax_delay_us; crash-dax is thin and not in the default set; pgbench-dax checks data once; nothing gates a push. TASK-112.6 (data loss when evacuations run late) needs a reproducer on main.
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
