---
id: TASK-26.19.9
title: 'Guest Fxfs: NFS export operations'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-15 08:19'
updated_date: '2026-10-15 11:09'
labels: []
dependencies: []
parent_task_id: TASK-26.19
priority: low
type: feature
ordinal: 72000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ext4 and btrfs provide export_operations (file handles), which NFS export and open_by_handle_at need.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 name_to_handle_at and open_by_handle_at work on Fxfs files
- [x] #2 A guest test covers a handle surviving inode eviction
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
6d0eec1: encode_fh (FILEID_INO64_GEN, gen 0), fh_to_dentry via ilookup or fxfs_getattr (drains queued unlinks; graveyard objects stale). No get_parent: directory handles can't be reconnected.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
File handles work for Fxfs inodes (name_to_handle_at/open_by_handle_at), verified by the fxfs-guest-checks(-dax) file_handles check, including after eviction, on GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
