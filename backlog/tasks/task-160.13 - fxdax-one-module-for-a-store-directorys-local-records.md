---
id: TASK-160.13
title: 'fxdax: one module for a store directory''s local records'
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
updated_date: '2026-11-05 16:24'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 584000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
remote-*, fence-*, origin-*, public-*, attached-bases and the boot profile each have their own format and atomic write (two copies of write-then-rename; the profile isn't fsynced); read errors handled three ways. Explains 3e6615852, TASK-79.82, 79.84, 79.95. Proposal: StoreDir with typed records, one durable write, one rule for read errors.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One durable write for every local record
- [ ] #2 The boot profile is fsynced
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
TASK-184 (2026-11-05): remote.rs's durable writes (markers, fences, claims) now share one helper, write_new, whose new file is the write's own ({name}.new-{random}); a shared {name}.new let two writers of one file at once truncate or remove each other's (ENOENT). The one durable write this task asks for must do the same: extents.rs's write_bases still writes {BASES_FILE}.new, which two ATTACHes of one store at once would share, and record_base reads, then writes, the list (a lost attachment); extent_store host.rs's describe has {DESCRIPTION}.new, which two deletions of one handle at once share. The read-then-write writers of a child's claim are TASK-186.
<!-- SECTION:NOTES:END -->
