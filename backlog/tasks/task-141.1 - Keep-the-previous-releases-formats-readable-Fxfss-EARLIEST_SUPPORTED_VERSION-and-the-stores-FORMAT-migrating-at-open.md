---
id: TASK-141.1
title: >-
  Keep the previous release's formats readable: Fxfs's
  EARLIEST_SUPPORTED_VERSION and the store's FORMAT, migrating at open
status: To Do
assignee: []
created_date: '2026-10-31 20:30'
labels: []
dependencies: []
parent_task_id: TASK-141
ordinal: 488000
---

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A release reads the store and Fxfs formats of the release before it
- [ ] #2 A golden store of the previous release, checked in, mounts, passes fsck and reads back as made, in CI
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
