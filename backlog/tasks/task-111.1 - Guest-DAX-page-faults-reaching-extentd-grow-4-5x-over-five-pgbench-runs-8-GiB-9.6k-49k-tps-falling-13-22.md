---
id: TASK-111.1
title: >-
  Guest DAX: page faults reaching extentd grow 4-5x over five pgbench runs (8
  GiB: 9.6k -> 49k), tps falling 13-22%
status: To Do
assignee: []
created_date: '2026-10-28 00:28'
labels: []
dependencies: []
parent_task_id: TASK-111
priority: high
ordinal: 415000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
In the sandbox team round 7 (TASK-111), with DAX the guest page faults that reach extentd grow from 7,986 to 32,532 a run over five 60 s pgbench runs with a 2 GiB window and from 9,557 to 49,090 with 8 GiB, while tps falls 22% and 13%. At 8 GiB the window is never full and reach MAPs stay ~4k, so window pressure (the explanation in TASK-109 for 2 GiB) does not account for it; our own 8 GiB runs also fell (1168/1063/960/1045/1016 at a84611804). extentd time per fault stays ~10 us; its memory climbs toward its 8 GiB budget (3.8 -> 7.8 GB). No DAX shows none of it. Cause not shown: candidates are pages the window lets go of, or extentd evicts, being faulted back in.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause of the growth is identified and recorded
- [ ] #2 Over five 60 s runs at 8 GiB, faults reaching extentd per run grow under 1.5x from run 1 to run 5
- [ ] #3 Run 5 tps is at least 90% of run 1 at 2 GiB and at 8 GiB
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
