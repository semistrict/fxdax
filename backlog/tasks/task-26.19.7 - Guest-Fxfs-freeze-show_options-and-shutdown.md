---
id: TASK-26.19.7
title: 'Guest Fxfs: freeze, show_options and shutdown'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-15 08:19'
updated_date: '2026-10-15 08:56'
labels: []
dependencies: []
parent_task_id: TASK-26.19
priority: low
type: feature
ordinal: 70000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ext4 and btrfs implement freeze_fs/unfreeze_fs (fsfreeze, snapshots of a quiesced filesystem), show_options (mount options in /proc/mounts) and shutdown (FS_IOC_SHUTDOWN for testing).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fsfreeze -f blocks writers until fsfreeze -u, with everything durable meanwhile
- [ ] #2 /proc/mounts shows the mount options
- [x] #3 Guest tests cover freeze and thaw
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
81cecd7: freeze_fs syncs durably, unfreeze_fs nothing. Guest check freeze (FIFREEZE/FITHAW) passes. Writers blocking while frozen not tested; show_options and shutdown not done.
<!-- SECTION:NOTES:END -->
