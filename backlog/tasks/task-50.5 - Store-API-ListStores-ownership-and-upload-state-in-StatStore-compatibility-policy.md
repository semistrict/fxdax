---
id: TASK-50.5
title: >-
  Store API: ListStores, ownership and upload state in StatStore, compatibility
  policy
status: In Progress
assignee: []
created_date: '2026-10-19 20:10'
updated_date: '2026-10-20 05:14'
labels: []
dependencies: []
parent_task_id: TASK-50
priority: medium
ordinal: 154000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
sandbox-host rebuilds its view of a host's stores after it restarts, and billing reads every store; neither should take a call per store. Fencing needs to know which placement holds a store, and stop deadlines and alerts need what is not yet uploaded. The gRPC definitions have no compatibility promise.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 ListStores returns every store the daemon serves, with StatStore's fields
- [ ] #2 StatStore reports the lease's token and when it was taken, the latest uploaded commit, and the bytes pending upload and the age of the oldest
- [ ] #3 A compatibility policy for the control API's protobuf definitions is documented
- [ ] #4 Tests cover each new field and call
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
