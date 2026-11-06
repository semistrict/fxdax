---
id: TASK-149.34
title: A connection's page-ins in flight are bounded by nothing it keeps within
status: To Do
assignee: []
created_date: '2026-11-06 03:59'
labels:
  - bug
  - extentd
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 644000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found looking around TASK-149.17's memory finding. A request's own copies are a piece at a time now, and its stream data within the connection's room; but a fault, or a read, of a cold extent pages it in, a 1 MiB part at a time with read-ahead (cache.rs read_pages, extent_frame::PART), holding a part's buffer while it reads, and a fault waiting for the pager holds no worker (TASK-92.20), so a VM process's faults on many cold extents at once page in as many parts at once as it has faults in flight (MAX_REQUESTS, 4096), uncounted. One cold 64 MiB READ held about 31 MiB past the room in what_requests_hold_is_within_the_room, which reads its extent once first for that. Unmeasured: what a storm of faults on distinct cold extents holds.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A storm of faults, and of reads, on distinct cold extents, as many as a connection keeps in flight, measured by the counted heap of extentd/tests/hostile.rs
- [ ] #2 What it holds is within a bound per connection, or the bound is the daemon's own, stated, and the hostile test checks it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
