---
id: TASK-111.2
title: >-
  Bench: one pgbench comparison entry point (windows none/2G/8G, 5 runs, extentd
  memory cap, bucket latency) with machine-readable per-run results
status: To Do
assignee: []
created_date: '2026-10-28 01:25'
labels: []
dependencies: []
parent_task_id: TASK-111
priority: medium
ordinal: 416000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sandbox team runs our pgbench comparison by hand and acts as our benchmark loop; regressions such as the 8 GiB collapse (TASK-109.2) reached them first. They ask for one entry point they and our CI can run, defaulting to their setup (TASK-111). Five runs is the minimum: the 8 GiB collapse appeared only from run 3.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One command runs no DAX, DAX 2 GiB and DAX 8 GiB, N runs each, with the extentd memory cap and bucket latency as options, defaulting to the sandbox setup in TASK-111
- [ ] #2 It writes JSON per run: init time, tps, latency percentiles, and the counter deltas of the run
- [ ] #3 It runs on a GCE nested host and on bare metal
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
