---
id: TASK-364.1
title: 'Whole-cell forks: every volume of a cell forked at one cut into a new cell'
status: To Do
assignee: []
created_date: '2026-11-12 02:16'
labels:
  - bucket
  - host
dependencies: []
parent_task_id: TASK-364
priority: medium
ordinal: 751000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The user chose whole-cell forks for cells (TASK-364, 2026-11-11): a cell of several volumes forked into a new cell, each volume a fork of its parent volume at one consistent point. TASK-364 makes every fork of a one-volume cell a new cell; a cell whose volumes are written by several processes of its host (extentd for VMs, fxdax-mount for FUSE) has no one place that can pause them all and cut them at once yet. Volumes are branches with logs of their own, so the cut is the holder pausing commits on every volume, forking each, and writing the new cell's object naming the children.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A control call forks a held cell into a new cell (of the same or another tenant), every volume of it forked, the new cell's object naming each child under its parent volume's name and kind
- [ ] #2 The cut is consistent: no volume commits between the first volume's fork and the last's, shown by a test that commits to two volumes concurrently and checks the children hold one moment of both
- [ ] #3 Volumes the forking process does not write (another process of the host) are paused through it or the fork is refused, shown by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
