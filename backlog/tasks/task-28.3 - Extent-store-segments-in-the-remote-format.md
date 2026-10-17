---
id: TASK-28.3
title: 'Extent store: segments in the remote format'
status: To Do
assignee: []
created_date: '2026-10-17 01:31'
updated_date: '2026-10-17 01:32'
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
