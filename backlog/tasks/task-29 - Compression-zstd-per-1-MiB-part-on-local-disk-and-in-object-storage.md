---
id: TASK-29
title: 'Compression: zstd per 1 MiB part, on local disk and in object storage'
status: To Do
assignee:
  - '@claude'
created_date: '2026-10-17 13:35'
labels: []
milestone: m-5
dependencies: []
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
priority: high
type: feature
ordinal: 94000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Extent data is stored and uploaded uncompressed, so the local disk and the bucket hold, and transfer, the guest raw bytes. doc-7 decision 7 ruled compression out because it breaks ranged reads that land straight in DAX pages. That no longer holds: the DAX window maps extentd memory, which the pager fills by reading the store (ExtentStore::map, StoredExtent::read, FxExtent), and never maps store files. So every byte on disk and in the bucket can be compressed, with decompression at page-in, at READ, and when fetching from the bucket.
Decided with the user on 2026-10-17:
- The guest keeps mapping extentd memory; all data on disk is compressed, and memory is the uncompressed working set.
- Each 1 MiB part (extent_frame::PART) is compressed with zstd on its own, so a ranged GET or a local read decompresses only what it needs.
- Local closed segments stay byte-identical to the uploaded objects (doc-7 decision 1).
zstd is BSD-3-Clause/GPL-2.0 dual-licensed, and the Rust zstd crate is MIT.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 doc-7 decision 7 and the Segments and Reading sections describe compressed parts
- [ ] #2 The subtasks are done
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
