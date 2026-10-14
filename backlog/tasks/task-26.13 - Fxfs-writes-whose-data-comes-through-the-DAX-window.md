---
id: TASK-26.13
title: 'Fxfs: writes whose data comes through the DAX window'
status: To Do
assignee: []
created_date: '2026-10-14 14:46'
labels: []
milestone: m-3
dependencies: []
parent_task_id: TASK-26
type: feature
ordinal: 49000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
For DAX writes (TASK-26.6, full DAX), Fxfs allocates a new extent for a file range and records it in the file's extent tree in a transaction, without writing data: the guest stores the data through the window (MAP_WRITE), and the next COMMIT seals the extent. Parts of the range the write doesn't cover are copied from the old data first. An inode's write stream extends the extent it has open while it stays open.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An Fxfs API allocates and records an extent for a file range without writing it, and a test writing through it (FakeDevice) reads the data back after remount
- [ ] #2 Partial blocks keep the data around them; fsck passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
