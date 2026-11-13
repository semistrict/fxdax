---
id: TASK-395.11
title: >-
  A writer records a fork across cells whose parent's state at the fork point is
  gone (sim restores seed 11)
status: To Do
assignee: []
created_date: '2026-11-13 15:51'
labels:
  - bug
  - bucket
  - collector
dependencies: []
parent_task_id: TASK-395
priority: medium
ordinal: 785000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the simulation's points_restored_on_many_hosts_at_once, seed 11 (restores profile), on the TASK-395.4 review fixes: branch 1/2/2's uploader acknowledged and recorded a fork at its segment 3 to point 1/23/23, which had no record, after the parent's segments 1 to 5 were gone (its checkpoints 0xb and later covering them), so the parent's state at the fork point could not be recovered: recovery::checkpoint_at, which recording a fork across cells now calls, failed Lost and failed the uploader. Before TASK-395.4's review the record was created all the same, naming a state no reader could recover (the sim passed: the child was no host's, deleted after). No pass kept the fork point: the bucket's checkpoints of the parent did not name the fork, only the uploader's state did, so how the uploader's state came to hold a fork the branch's checkpoints don't is the question (a stale state, a WAL rebased, a fork segment of another history). Since the review, record_fork records such a child deleted and warns (branch::record_fork). Reproduce: SIM_PROFILE=restores SIM_SEED=11 cargo nextest run --cargo-profile sim -p extent_store --run-ignored only -E 'test(=sim::one)', and look for the warning.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause is found and fixed or shown benign; the deleted-record path either goes or is tested
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
