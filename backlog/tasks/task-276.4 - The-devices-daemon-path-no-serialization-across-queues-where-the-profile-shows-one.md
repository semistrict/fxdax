---
id: TASK-276.4
title: >-
  The device's daemon path: no serialization across queues where the profile
  shows one
status: To Do
assignee: []
created_date: '2026-11-09 03:54'
labels:
  - perf
dependencies: []
parent_task_id: TASK-276
type: enhancement
ordinal: 706000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
All queue workers of an extent device share one DaemonConnection: one client receiving thread completes every queues requests, the daemon serves at most WORKERS (8) of a connections requests at once, and the transfer area has 8 slots. Whether any of these caps scaling is for the phase-1 profile to say.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The phase-1 profile says whether the daemon path serializes, and where
- [ ] #2 If it does, the change removes it, keeping extentds per-store invariants (doc-4, doc-13), with Shuttle/concurrency tests; if not, the task records why it was skipped
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
