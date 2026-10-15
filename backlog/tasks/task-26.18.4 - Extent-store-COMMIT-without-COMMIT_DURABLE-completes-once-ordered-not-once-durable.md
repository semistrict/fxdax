---
id: TASK-26.18.4
title: >-
  Extent store: COMMIT without COMMIT_DURABLE completes once ordered, not once
  durable
status: To Do
assignee: []
created_date: '2026-10-15 08:19'
labels: []
dependencies: []
parent_task_id: TASK-26.18
priority: medium
type: enhancement
ordinal: 58000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-4 lets a plain COMMIT complete once its order is fixed (doc-2 requirement 2: fsync keeps ordering, a crash may lose a suffix); only COMMIT_DURABLE must wait. The host makes every barrier durable (fdatasync of segments and index), so WRITE requests with WRITE_BARRIER take 5-20 ms and every guest fsync pays a host fsync (create-fsync, fsync-append). Recovery then needs data checksums or another way to find a torn epoch.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A plain COMMIT and WRITE_BARRIER complete without waiting for fdatasync
- [ ] #2 COMMIT_DURABLE still waits for durability
- [ ] #3 A crash test shows the store recovers to a prefix of committed epochs
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
