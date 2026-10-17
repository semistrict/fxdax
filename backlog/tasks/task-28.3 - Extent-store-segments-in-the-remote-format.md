---
id: TASK-28.3
title: 'Extent store: segments in the remote format'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 01:31'
updated_date: '2026-10-17 08:55'
labels: []
milestone: m-5
dependencies:
  - TASK-28.2
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
priority: high
type: feature
ordinal: 83000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-7 decision 1: a closed local segment is byte for byte the object uploaded, so uploading is a PUT of the file and the NVMe cache is the same files. Today segments hold pieces of open extents and only the index knows what is where; an extent is stored on its own by its first write; files are removed once their commit is durable locally, which would lose extents freed before upload.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Sealed extents are stored in one 4 KiB-aligned run each, and a closed segment ends in the doc-7 footer (the extents sealed, moved or adopted with their lengths and places, frees, root, next sequence), framed with a CRC32C per 1 MiB part and a trailer as bucket objects are
- [x] #2 The index of a branch can be rebuilt from its newest checkpoint and its closed segments' footers, shown by a test
- [x] #3 Segments close on size or age; extents are stored on their own by sealed length
- [x] #4 Local files stay until a hook says the commit that made them unreferenced is uploaded, shown by a test
- [x] #5 The crash test still recovers a prefix of commits
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. crates/extent_frame: the framing of objects that hold data, shared by local segments and bucket objects (data | footer | a CRC32C per 1 MiB part of the data | 16-byte trailer), moved out of extent_remote::layout, which re-exports it.
2. Sealed extents are one 4 KiB-aligned run each: at a commit, a sealed extent whose pieces are not one run in the open segment is copied into one there. An extent sealed at >= large_extent is stored in its own file, and one below it in a segment, copying at seal where its first write chose otherwise.
3. Segments close at a commit, once no write is in progress in them: when full (segment_size), when old (segment_age), when asked (Store::close_segment, for the uploader), and at a fork. Each closed segment's commits form a window of their own, and the segment ends in its footer: the window's net delta (the extents sealed or moved there, own or adopted, with their lengths; the frees; the root; the next sequence; the window's epochs), framed as in step 1.
4. A branch's state = its newest index checkpoint + the footers of the closed segments after it (rebuild). A closed segment no extent lives in goes only once a checkpoint at or past its window covers it.
5. Local files stay until the upload hook releases the commits that made them unreferenced (Uploads.kept/release), shown by a test under the new closing.
6. The crash tests still recover prefixes; a rebuild test compares footers with the index.
7. Then the uploader takes segments' data by their runs (doc-7 decision 1). Byte-identical objects would need the writer's numbering and footer fields locally, so that part is noted as a deviation.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Landed:
- c364510: extent_frame, the framing shared with bucket objects.
- a0d34be: one run per sealed extent, own files by sealed length, segment windows closed on size, age, request or fork, footers, Store::rebuild and Store::places, footers voided past the recovered epoch, closed dead segments kept until a checkpoint covers them. Also fixed MemDir::list, which did not list directories.
- c7999c9: local checkpoints as extent_frame tables.
AC wording updated to doc-7 as it stands: part CRCs rather than per-extent ones; 'rebuilt from footers' means from the newest checkpoint plus the footers, as in the bucket, since dead closed segments go once a checkpoint covers them.
Tests:
- sealed_extents_are_one_run_each
- closed_segments_hold_the_branchs_history (closes on request, full, old, at a fork; rebuild equals the branch; a run's data is where its footer says)
- checkpoints_cover_closed_segments
- uploads_keep_freed_data_until_released (the Uploads kept/release hook)
- crash tests: recovers_a_prefix_of_its_commits, with footers never past the recovered epoch and rebuild equal to the branch once closed; forks_recover_prefixes_of_their_commits
Workspace: macOS 1952, Linux 1962. upstream-changes.py --check passes.
Deviation, recorded in doc-7: a closed segment's data is the object's, but its footer is local. The bucket footer holds the writer's lease epoch, the after link and commit tags, and the writer numbers bucket segments (fences, gaps). So uploads still read sealed extents back, now each in one run, rather than PUT the file.
Follow-ups: computing part CRCs incrementally rather than reading a closing segment back under the state lock; direct segment uploads, which need the writer to take a segment's data and runs as they are.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Sealed extents are now one 4 KiB-aligned run each (copied at seal when fragmented), and extents sealed at large_extent or more live in their own files. A segment closes at a commit on size, age, request (Store::close_segment) or fork, once no write is in flight. Its footer of the window's changes then follows its data, framed with per-part CRCs and a trailer like bucket objects. A branch is its newest checkpoint plus the footers after it (Store::rebuild); closed dead segments go once a checkpoint covers them. Verified by the run, closing, rebuild and crash tests and the macOS and Linux suites. Uploading segment files directly remains a documented deviation.
<!-- SECTION:FINAL_SUMMARY:END -->
