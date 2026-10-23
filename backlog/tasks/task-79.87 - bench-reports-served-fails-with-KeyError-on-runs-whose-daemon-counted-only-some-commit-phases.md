---
id: TASK-79.87
title: >-
  bench-report's served() fails with KeyError on runs whose daemon counted only
  some commit phases
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 09:16'
updated_date: '2026-10-23 09:17'
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
- [ ] #1 Reports of runs with only some commit phases counted succeed and show those phases, pinned by scripts/tests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
