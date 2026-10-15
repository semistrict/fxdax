---
id: TASK-26.19.1
title: 'Guest Fxfs: O_TMPFILE (inode_operations.tmpfile)'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-15 08:19'
updated_date: '2026-10-15 08:56'
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
- [ ] #1 open(dir, O_TMPFILE) creates an unnamed file that is freed on close
- [x] #2 linkat() of an O_TMPFILE file gives it a name
- [ ] #3 A guest test covers both, and a crash leaves no leaked object
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
81cecd7: .tmpfile creates the object in the graveyard (create_child_unnamed_temporary_file); linkat moves it out (remove_from_graveyard). Guest check tmpfile passes on GCE (linkat path verified; freeing of a never-named file not observed by a test; crash leak not tested).
<!-- SECTION:NOTES:END -->
