---
id: TASK-26.19.2
title: 'Guest Fxfs: fallocate (allocate, punch hole, zero range, keep size)'
status: To Do
assignee: []
created_date: '2026-10-15 08:19'
labels: []
dependencies: []
parent_task_id: TASK-26.19
priority: medium
type: feature
ordinal: 65000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ext4 and btrfs implement file_operations.fallocate; Fxfs (guest) does not. Fxfs supports preallocation natively (copy-on-write over extents, TASK-26.3 notes fallocate no longer guarantees space on a full store).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fallocate modes 0, FALLOC_FL_KEEP_SIZE, PUNCH_HOLE and ZERO_RANGE work on regular files
- [ ] #2 Unsupported modes fail with EOPNOTSUPP
- [ ] #3 Guest tests cover each mode
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
