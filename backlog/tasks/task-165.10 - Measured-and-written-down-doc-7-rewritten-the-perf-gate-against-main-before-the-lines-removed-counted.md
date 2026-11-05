---
id: TASK-165.10
title: >-
  Measured and written down: doc-7 rewritten, the perf gate against main before,
  the lines removed counted
status: To Do
assignee: []
created_date: '2026-11-05 03:17'
labels: []
dependencies:
  - TASK-165.9
documentation:
  - doc-14
parent_task_id: TASK-165
priority: medium
ordinal: 632000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 10. The restructure trades local copies for the part cache and the bucket; its cost must be measured by the gate the DAX work used, and doc-7 still describes local lineages, local checkpoints and markers.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 doc-7 describes the host as it is: WAL, part cache, one recovery; no local lineage or marker remains in it
- [ ] #2 The perf gate on GCE (pgbench tps, first read of /usr, fork and restore latency, memory per VM) against main before the restructure, recorded in docs/measurements; a regression past noise is a task or a dead end
- [ ] #3 The lines removed and added, by module, recorded in TASK-165's notes against doc-14's estimates; the review's section 2 marked done
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
