---
id: TASK-29.2
title: 'Extent store: closed segments and own extents compressed; reads decompress'
status: To Do
assignee:
  - '@claude'
created_date: '2026-10-17 13:35'
labels: []
dependencies:
  - TASK-29.1
parent_task_id: TASK-29
priority: high
type: feature
ordinal: 96000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
With framing that compresses parts (TASK-29.1), the local store compresses a segment when it closes, and an extent stored on its own when it is sealed. An open segment, whose extents are still being written, stays raw until it closes.
Today several paths assume an extent bytes sit at a fixed offset in a file:
- Store::locate and Part::Data, which fxdax ImageDevice (crates/fxdax/src/storage/image.rs) uses to pread straight into kernel buffers with O_DIRECT;
- read_into_entry and read_locked;
- cleaning;
- the sealing copy;
- Store::rebuild from footers.
These must read and decompress parts instead. The guest DAX window is unaffected: it maps extentd memory, filled through StoredExtent::read.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Closed segments and own extents are stored compressed in the TASK-29.1 framing, and a closed segment is byte-identical to the object uploaded for it
- [ ] #2 Every read path (Store::read, locate users including ImageDevice, cleaning, rebuild, read_kept) returns the same bytes as before, shown by the existing store and fxdax tests and a test that compares raw and compressed stores
- [ ] #3 The crash test still recovers a prefix of commits, and the guest suites pass (fxfs-guest, -dax, -checks, -checks-dax, -crash, -crash-dax, extent-guest, pjdfstest)
- [ ] #4 A store full of compressible data takes correspondingly less disk, shown by a test of Usage
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
