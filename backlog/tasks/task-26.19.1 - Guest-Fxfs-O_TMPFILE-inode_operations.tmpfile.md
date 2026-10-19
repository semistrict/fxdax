---
id: TASK-26.19.1
title: 'Guest Fxfs: O_TMPFILE (inode_operations.tmpfile)'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-15 08:19'
updated_date: '2026-10-19 04:05'
labels: []
dependencies: []
parent_task_id: TASK-26.19
priority: medium
type: feature
ordinal: 64000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ext4 and btrfs implement .tmpfile; Fxfs does not, so open(O_TMPFILE) fails with EOPNOTSUPP. Fxfs already keeps unlinked-but-open objects in its graveyard, which is what an anonymous temporary file is.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 open(dir, O_TMPFILE) creates an unnamed file that is freed on close
- [x] #2 linkat() of an O_TMPFILE file gives it a name
- [x] #3 A guest test covers both, and a crash leaves no leaked object
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
81cecd7: .tmpfile creates the object in the graveyard (create_child_unnamed_temporary_file); linkat moves it out (remove_from_graveyard). Guest check tmpfile passes on GCE (linkat path verified; freeing of a never-named file not observed by a test; crash leak not tested).

Verified on GCE (29ca40b): fxfs-guest-checks tmpfile: never named, the object count back as it was once closed (graveyard batch); fxfs-guest-crash: an unnamed file of 24 MiB, durable, open as the VM resets, freed once the store is mounted again (used bytes back within metadata of what names hold).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
O_TMPFILE creates an unnamed file in the graveyard, linkat names it, and closing it frees it; a crash leaves none: shown by fxfs-guest-checks and fxfs-guest-crash on GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
