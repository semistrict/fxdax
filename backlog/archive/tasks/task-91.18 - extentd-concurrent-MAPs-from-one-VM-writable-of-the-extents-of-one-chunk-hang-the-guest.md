---
id: TASK-91.18
title: >-
  extentd: concurrent MAPs from one VM, writable, of the extents of one chunk,
  hang the guest
status: To Do
assignee: []
created_date: '2026-10-23 20:46'
updated_date: '2026-10-23 22:12'
labels:
  - bug
dependencies: []
parent_task_id: TASK-91
priority: medium
ordinal: 362000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found while adding MAP_MANY (TASK-91.4): with the VMM sending a MAP_MANY's maps to the daemon at once (try_join_all of map_async on one connection), fxfs-guest-checks-dax hung in 'freeze' and fxfs-guest-pgbench-dax in initdb (postgres blocked > 120 s, no stack). Mapping them in order passes. A chunk's pieces include writable maps of the open staging extent, which a first MAP opens; two at once likely race the open or a lock in extentd's map path. Two vCPUs mapping pieces of one open extent at once could reach the same state today. Kernel vmlinux-drv2, GCE r63, 2026-10-23.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test with concurrent writable MAPs of one open extent, red before the fix
- [ ] #2 Concurrent MAPs complete
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Not a bug in extentd: the hang was the guest driver's (vext_wait took the completion with try_wait_for_completion, then waited for it again), fixed before 06dd6a64. With concurrent MAP_MANY maps restored, fxfs-guest-checks-dax and fxfs-guest-pgbench-dax pass (GCE r63, vmlinux-drv3).
<!-- SECTION:NOTES:END -->
