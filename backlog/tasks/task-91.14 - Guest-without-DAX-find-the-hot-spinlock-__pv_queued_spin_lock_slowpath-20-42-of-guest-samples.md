---
id: TASK-91.14
title: >-
  Guest without DAX: find the hot spinlock (__pv_queued_spin_lock_slowpath
  20-42% of guest samples)
status: To Do
assignee: []
created_date: '2026-10-23 18:01'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: medium
ordinal: 352000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Round 6: in the default no-DAX run __pv_queued_spin_lock_slowpath was 20% of guest samples, 42% with synchronous_commit=off where the guest is CPU-bound; the sandbox team's sampler has no stacks. A guest perf with call graphs on that run would say which lock.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The lock is identified with a guest profile with call graphs
- [ ] #2 Its contention cut, or why it can't be stated
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
