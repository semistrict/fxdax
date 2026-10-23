---
id: TASK-79.87
title: >-
  bench-report's served() fails with KeyError on runs whose daemon counted only
  some commit phases
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-23 09:16'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 346000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found reviewing TASK-79.64 (round 2c), pre-existing. served() in the bench report read every commit phase, and failed with KeyError 'commit-written-bytes' on runs whose daemon did not count them all (gce-fs-bench-phases-2026-10-20T2352Z, gce-fs-bench-prof-2026-10-21T0020Z). Fixed on r2c/scripts: it reports only the phases counted.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Reports of runs with only some commit phases counted succeed and show those phases, pinned by scripts/tests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
bench-report.py's served() read every commit phase from the daemon's stats. On runs whose daemon did not count the writing of open extents separately, such as gce-fs-bench-phases-2026-10-20T2352Z and gce-fs-bench-prof-2026-10-21T0020Z, it failed with KeyError 'commit-written-bytes'. It now reports only the phases the daemon counted (ce52e19f). Both runs now report, and the other 99 runs' reports are unchanged. test_commits_by_the_phases_an_older_daemon_counted raised KeyError before the fix and now passes, with the scripts' 79 tests, on macOS and Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
