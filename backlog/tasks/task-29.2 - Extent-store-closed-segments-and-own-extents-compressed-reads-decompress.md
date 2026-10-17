---
id: TASK-29.2
title: 'Extent store: closed segments and own extents compressed; reads decompress'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 13:35'
updated_date: '2026-10-17 15:10'
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
- [x] #1 Closed segments and own extents are stored compressed in the TASK-29.1 framing, and a closed segment is byte-identical to the object uploaded for it
- [x] #2 Every read path (Store::read, locate users including ImageDevice, cleaning, rebuild, read_kept) returns the same bytes as before, shown by the existing store and fxdax tests and a test that compares raw and compressed stores
- [x] #3 The crash test still recovers a prefix of commits, and the guest suites pass (fxfs-guest, -dax, -checks, -checks-dax, -crash, -crash-dax, extent-guest, pjdfstest)
- [x] #4 A store full of compressible data takes correspondingly less disk, shown by a test of Usage
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Open data raw in open-segments/ and open-extents/; closing a segment and sealing an own extent write a framed file in segments/ or extents/, the raw one doomed once the commit is durable.
2. Recovery: a raw segment whose framed file is missing, torn, or has a footer past the recovered epoch is framed again with no footer; Lineage::open removes unclaimed raw files and all open-extents files.
3. Reads: Source (raw or framed), Part::Data/Packed/Unpacked/Zeros; a small LRU of unpacked parts; ImageDevice reads packed parts' stored bytes through its I/O engine and unpacks them.
4. Usage.disk_bytes; tests for compression, crash during close, part kinds.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Byte-identical (user asked 2026-10-17): the local store closes segments with the bucket's footer (extent_frame::segment::Footer, moved from extent_remote::layout). The uploader gives the store a Footing (writer lease epoch, next segment number, segment to follow, sequence floor); the store numbers/chains its segments so, tags commits tag(writer, epoch), names own extents at extent_key(writer, id), and pins each closed segment until uploaded (Store::closed / Store::uploaded). Store::close_now closes between commits once no write is in progress (writes wait), durably, removing the raw file. Local checkpoints record the segment open then; rebuild follows the after-chain from it; adopt makes the next commit checkpoint (footers don't name remote-data extents). Recovery re-frames with no footer any segment whose footer names non-durable commits, from its framed data, or the raw file if the framed one is torn. Tests: segments_close_for_the_writer_of_their_footing (extent_store), writers_upload_segments_stores_closed (extent_remote), uploaded_segments_are_the_stores_files (fxdax: every bucket segment with commits == a local segment file, own extent objects == local extent files).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Closed segments and sealed own extents are framed and compressed; open data is raw in open-segments/ and open-extents/. Closed segments carry the bucket's footer, numbered for the uploader's writer (Footing), so the uploaded object is the local file byte for byte. Reads decompress touched parts (LRU of unpacked parts); ImageDevice reads packed parts through its I/O engine. Verified: extent_store tests incl. crash tests, compression/usage/close tests, fxdax uploaded_segments_are_the_stores_files, Lima guest suites (fxfs-guest, -dax, -checks, -checks-dax, -crash, -crash-dax, -format, extent-guest) and pjdfstest with and without DAX.
<!-- SECTION:FINAL_SUMMARY:END -->
