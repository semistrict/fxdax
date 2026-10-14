---
id: TASK-26.11
title: 'doc-4: MAP_WRITE opens and grows the guest''s extents'
status: Done
assignee: []
created_date: '2026-10-14 14:46'
updated_date: '2026-10-14 14:47'
labels: []
milestone: m-3
dependencies: []
parent_task_id: TASK-26
type: task
ordinal: 47000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The user chose full DAX with writes through the window (2026-10-14), doc-2's provisional write path. doc-4 lets MAP_WRITE map an open extent of the guest's branch writably (DAX_WRITE), but not how an extent that has no data yet opens, or how long it becomes. Specify: MAP_WRITE of an extent of the guest's branch that is open or new opens it, its length the furthest end mapped or written, zero-filled; the next barrier seals it and its writable mappings become read-only; INFO and FREE apply as to any extent.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 doc-4 specifies MAP_WRITE on new and open extents, their length and contents, and sealing's effect on writable mappings
- [x] #2 The device and driver constants match it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
doc-4 specifies MAP_WRITE on new and open extents (opening, length, zero fill, EPERM) and sealing's effect on writable mappings. The constants (VIRTIO_EXTENT_F_DAX_WRITE, VIRTIO_EXTENT_MAP_WRITE) already match in the driver's header and libkrun's device.
<!-- SECTION:FINAL_SUMMARY:END -->
