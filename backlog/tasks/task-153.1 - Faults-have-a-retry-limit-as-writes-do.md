---
id: TASK-153.1
title: 'Faults have a retry limit, as writes do'
status: To Do
assignee: []
created_date: '2026-11-01 22:52'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: high
type: task
ordinal: 545000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Fault, semaphore and seal form a lock cycle broken only by retries, and fxfs_dax_do_fault has no limit, where a write has FXFS_DAX_MAX_DROPS. Explains TASK-79.12, TASK-113, bdc653701.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A fault that can not get on fails and says so, rather than going round for ever
- [ ] #2 A test shows it, red with the limit taken out
- [ ] #3 The sandbox benchmark mapped reads are not slowed: pgbench-dax and checks-dax pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
