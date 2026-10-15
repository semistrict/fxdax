---
id: TASK-26.19.2
title: 'Guest Fxfs: fallocate (allocate, punch hole, zero range, keep size)'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-15 08:19'
updated_date: '2026-10-15 11:18'
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
- [x] #1 fallocate modes 0, FALLOC_FL_KEEP_SIZE, PUNCH_HOLE and ZERO_RANGE work on regular files
- [x] #2 Unsupported modes fail with EOPNOTSUPP
- [x] #3 Guest tests cover each mode
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
81cecd7: modes 0 and KEEP_SIZE; others EOPNOTSUPP (verified by guest check fallocate). PUNCH_HOLE/ZERO_RANGE not done (need DAX layout breaking, doc-5).

7b7eb99: PUNCH_HOLE and ZERO_RANGE: whole pages' blocks freed in Fxfs (DataObjectHandle::zero), partial pages zeroed through the DAX window (DAX) or Fxfs (page cache), window ranges dropped like truncate. Guest checks (DAX and not) pass on GCE: bytes, SEEK_HOLE over the freed page, zero range growing the file, host read.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fallocate supports modes 0, KEEP_SIZE, PUNCH_HOLE and ZERO_RANGE (others EOPNOTSUPP); verified by fxfs-guest-checks and -checks-dax on GCE, including the host's read of the result. No space is reserved (extent store).
<!-- SECTION:FINAL_SUMMARY:END -->
