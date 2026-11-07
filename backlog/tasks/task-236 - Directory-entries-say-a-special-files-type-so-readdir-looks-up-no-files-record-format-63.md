---
id: TASK-236
title: >-
  Directory entries say a special file's type, so readdir looks up no file's
  record (format 63)
status: Done
assignee: []
created_date: '2026-11-07 20:39'
updated_date: '2026-11-07 23:02'
labels:
  - fxfs
  - format
dependencies: []
ordinal: 673000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-149.36 made readdir report FIFOs, sockets and devices as what they are by looking up each file's record, because Fxfs's entries said only file, directory or symlink: one tree lookup per file on every listing, in the guest too. The user chose the on-disk format change over the lookup (OPEN_QUESTIONS, 2026-11-07). fxdax migrates no format: every store made before it is unreadable after.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A directory entry of a special file says which (FIFO, character device, block device, socket), set as it is made or linked, kept across rename
- [x] #2 fxfs-posix readdir reads no file record to type an entry
- [x] #3 fsck reports an entry whose type differs from its object's
- [x] #4 The on-disk format is 63, and the model test, fsck tests and FUSE tests pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
ObjectDescriptorV63 gains Special(SpecialFileV63); every versioned type holding an ObjectValue becomes V63 and LATEST_VERSION 63. Directory::create_child_node takes the descriptor; fxfs-posix create and link derive it from the file's mode (ObjectDescriptor::of_file), rename carries it. readdir maps the descriptor alone; node_type shares the mapping. fsck keeps each file object's expected descriptor and reports ConflictingTypeForLink on a mismatch.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Validation: macOS workspace 2107/2107; model test hunt FXDAX_MODEL_CASES=3000 46/46; mutants.sh: the three readdir/create mutants rewritten to the new code, plus a-special-files-entry-says-file, all four found by the model test; cfg(fuzz) build checks; scripts/upstream-changes.py --check passes. Linux: CI 42417651745 (land-format63) green on every job, test (workspace) and test (guest) included, and CI 42420161055 (land-all3, landed 878aa8848). No GCE run of its own: CI's Linux workspace and guest jobs stood in for DoD #1's GCE.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A special file's directory entries say which it is (ObjectDescriptor::Special: FIFO, char device, block device, socket), set on create and link from its mode and kept by rename, so fxfs-posix readdir types each entry from the entry alone, no record lookup. fsck reports an entry whose type differs from its object's mode. On-disk format 63; format-62 stores are refused. Tests: readdir_lists_special_files_as_what_they_are (create, link, rename, fsck), test_conflicting_special_file_link_types, test_create_child_node_untimed; model test, mutants and CI green. Landed in 878aa8848.
<!-- SECTION:FINAL_SUMMARY:END -->
