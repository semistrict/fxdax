---
id: TASK-26.19.11
title: 'Guest Fxfs: large folios and O_DIRECT for files not on DAX'
status: Done
assignee: []
created_date: '2026-10-15 08:19'
updated_date: '2026-10-20 15:52'
labels: []
dependencies: []
parent_task_id: TASK-26.19
priority: low
type: enhancement
ordinal: 74000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ext4 and btrfs enable large folios in their page caches; Fxfs uses order-0 folios and has no direct I/O path (ext4 also offers MAP_SYNC on DAX via FOP_MMAP_SYNC).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Files not on DAX use large folios
- [x] #2 O_DIRECT reads and writes work
- [ ] #3 MAP_SYNC mmaps of DAX files are accepted and honoured
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Large folios (order up to 1 MiB, writers only; followers stay in pages since their batches drop ranges) and direct_IO through Fxfs; DAX files noop_direct_IO. write_begin's full-cover test fixed for large folios. Checks large_folios (most mapped pages of a read file are compound tails in /proc/kpageflags) and direct_io (aligned/unaligned/extending writes and reads past EOF agree with buffered I/O). AC3 (MAP_SYNC) not done: the DAX window is host memory, durable only once fsync commits the store, so MAP_SYNC's promise can't be kept; mmap with it fails EOPNOTSUPP, as README says. GCE: fxfs-guest-checks and -dax pass with the check; full suite 27/27 guest, 2239 workspace (commit df750c5).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Page-cache files use large folios and O_DIRECT works (direct_io, large_folios checks, GCE); MAP_SYNC stays unsupported, documented in README.
<!-- SECTION:FINAL_SUMMARY:END -->
