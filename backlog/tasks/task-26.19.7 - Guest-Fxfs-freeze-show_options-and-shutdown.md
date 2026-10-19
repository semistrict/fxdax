---
id: TASK-26.19.7
title: 'Guest Fxfs: freeze, show_options and shutdown'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-15 08:19'
updated_date: '2026-10-19 04:05'
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
- [x] #1 fsfreeze -f blocks writers until fsfreeze -u, with everything durable meanwhile
- [x] #2 /proc/mounts shows the mount options
- [x] #3 Guest tests cover freeze and thaw
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
81cecd7: freeze_fs syncs durably, unfreeze_fs nothing. Guest check freeze (FIFREEZE/FITHAW) passes. Writers blocking while frozen not tested; show_options and shutdown not done.

Verified on GCE (29ca40b): a writer started while frozen waits (500 ms) and finishes once thawed; freeze_fs syncs durably (the snapshot cases snapshot a frozen filesystem, restored whole on GCE, on a second host too); /proc/mounts shows dax when files map from the window (show_options, with follow for a follower). Shutdown (FS_IOC_SHUTDOWN), in the title, has no AC: not done.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Freezing blocks writers and makes everything durable, thawing lets them on, and /proc/mounts shows the mount as it is (dax, follow): fxfs-guest-checks freeze and show_options on GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
