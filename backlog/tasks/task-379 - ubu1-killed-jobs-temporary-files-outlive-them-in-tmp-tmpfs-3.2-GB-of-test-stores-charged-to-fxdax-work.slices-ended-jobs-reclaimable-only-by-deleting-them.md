---
id: TASK-379
title: >-
  ubu1: killed jobs' temporary files outlive them in /tmp (tmpfs): 3.2 GB of
  test stores charged to fxdax-work.slice's ended jobs, reclaimable only by
  deleting them
status: To Do
assignee: []
created_date: '2026-11-12 10:51'
updated_date: '2026-11-12 11:00'
labels:
  - bug
  - ubu1
dependencies: []
priority: medium
type: bug
ordinal: 761000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen 2026-11-12 while fixing TASK-378. fxdax-work.slice held 4.0 GB with no job of its own running: 3.2 GB shmem (memory.stat) in 110 dying memcgs, ended fxjob scopes. /tmp is a 14 GB tmpfs with 3.8 GB used, most of it tempfile TempDirs of fxdax tests (/tmp/.tmpXXXXXX with store/ and bucket/, e.g. .tmpJCnhSf 1.1 GB, .tmpLAS3YT 0.9 GB, .tmpyS27AI 0.7 GB, from 2026-11-11/12) whose processes are gone: tests killed (OOM kills, timeouts, cancelled runs) never drop their TempDir. tmpfs pages are reclaimed only by swap; those jobs ran with MemorySwapMax=0 (before TASK-378), so they stay charged to work, ~3 GB of work's room beside a measurement, until someone deletes the files. TASK-331 is the same leak for scripts/test-e2e.sh's cache; TASK-316 noted /tmp near full. Structural fix to weigh: fxjob gives each job a TMPDIR of its own (on disk, not tmpfs, or removed with the job at reap), so nothing a job leaves outlives it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A job killed with SIGKILL leaves none of its temporary files on ubu1
- [ ] #2 fxdax-work.slice holds no memory of ended jobs once they are reaped
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-12, the coordinator cleaned ubu1's /tmp: removed 1,516 test temp dirs (.tmp*, tmp.*, fxdax-*) older than 6 h, none open by any process nor any process's cwd; 1,035 of them root's. /tmp went from 3.7G to 2.0G used.
Three of them (.tmpKHntmR, .tmpy4Rk48, .tmprPqkTq, made 05:59 that day) each held a dead fuse.fxdax mount of a killed test ('Transport endpoint is not connected'): lazily unmounted, then removed. So a killed FUSE test leaves its mount behind as well as its files.
Suggested fix: fxjob gives each job a TMPDIR of its own, which tests make their temp dirs under; at the job's end its reap lazily unmounts any fuse.fxdax mount under it and removes it.
<!-- SECTION:NOTES:END -->
