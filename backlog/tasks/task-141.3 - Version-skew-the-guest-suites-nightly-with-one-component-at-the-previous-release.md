---
id: TASK-141.3
title: >-
  Version skew: the guest suites nightly with one component at the previous
  release
status: To Do
assignee: []
created_date: '2026-10-31 20:30'
labels: []
dependencies: []
parent_task_id: TASK-141
ordinal: 490000
---

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Daemon N with VMM N-1 passes the guest suites
- [ ] #2 VMM N with guest kernel N-1 passes the guest suites
- [ ] #3 A VMM's optional requests are listed, each tolerating EINVAL
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
