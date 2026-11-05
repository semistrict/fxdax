---
id: TASK-160.23
title: 'fxfs-posix: each operation declares what it touches'
status: To Do
assignee: []
created_date: '2026-11-03 13:09'
updated_date: '2026-11-05 03:06'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 594000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every method remembers by hand whether to drain unlinks, whether the failed-batch gate applies (only transaction_with checks it, crates/fxfs-posix/src/lib.rs:1732-1737), and whether to borrow metadata space (decided at five sites). Explains TASK-79.21, 26.19.9, 82, 60. Proposal: self.begin(Op::...) returning the guard and options from one table.

At 9f81f6018: dax_map_write and dax_update_size, which the review listed among those bypassing the gate, went with writable DAX (37c331a6c). Bypassing it now: clone_range (lib.rs:1026, new_transaction directly), write (677), write_dirty (702), write_many (726, DAX writeback's path, through the handle's own transactions), allocate (466), truncate (setattr, 1580-1591) and xattr writeback (write_xattrs, 1082).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One entry per operation
- [ ] #2 The TASK-79.21 cases as tests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
