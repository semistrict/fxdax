---
id: TASK-350.6
title: 'Bench: metadata-heavy work on a warm cache'
status: To Do
assignee: []
created_date: '2026-11-11 10:56'
labels: []
dependencies:
  - TASK-350.1
parent_task_id: TASK-350
priority: medium
ordinal: 710000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Fxfs in the guest trails ext4 by 1.5-2x on metadata: unlink 2.15x, find cold 2.06x, stat cold 1.94x, unlink-parallel 1.91x, rename 1.5x (docs/measurements/gce-fs-bench-2026-10-31-refresh). The sandboxes run ext4 in the guest.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 npm install of a fixed lockfile, rm -rf of the result, git status and git checkout on a large repository (e.g. Linux), find over it, warm and cold, against the JuiceFS baseline
- [ ] #2 Reports time and guest CPU per workload
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
