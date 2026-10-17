---
id: TASK-28.3
title: 'Extent store: segments in the remote format'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-17 01:31'
updated_date: '2026-10-17 08:32'
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
- [ ] #1 Sealed extents are stored in one 4 KiB-aligned run each, and a closed segment ends in the doc-7 footer (extents with CRC32C per extent and per 64 KiB, own extents, frees, root, next sequence, trailer)
- [ ] #2 The index of a branch can be rebuilt from its segments footers alone, shown by a test
- [ ] #3 Segments close on size or age; extents are stored on their own by sealed length
- [ ] #4 Local files stay until a hook says the commit that made them unreferenced is uploaded, shown by a test
- [ ] #5 The crash test still recovers a prefix of commits
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
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
