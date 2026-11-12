---
id: TASK-389
title: >-
  The CI soak at its default 300 s fails on ubu1 while a bench job runs: it
  writes 48 MiB/s, past work's 32 MB/s disk cap, so no store reaches the lag
  bound
status: To Do
assignee: []
created_date: '2026-11-12 19:27'
labels:
  - testing
  - ubu1
dependencies: []
priority: low
ordinal: 766000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-382 (2026-11-12): the guest CI soak's 'commits waited at the lag bound' check failed on ubu1 as ordinary fxjob work while another job held fxjob --bench, because TASK-374's cap (fxdax-work.slice, 32 MB/s of writes on /mnt/fxdax-scratch while a bench job runs) kept the soak's 48 MiB/s from filling any store to the bound. The 1,200 s run passed. A check that depends on the host's disk throughput fails for the environment, not the code: either the soak sizes its write rate or its duration from the throughput it measures, or it refuses to start (saying why) when the disk is capped, or host-remote runs it in the bench slot.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The CI soak passes or refuses with a clear reason on ubu1 while a bench job holds the slot, never a false check failure
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
