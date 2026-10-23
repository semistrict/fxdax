---
id: TASK-91.12
title: >-
  Guest Fxfs without DAX writes 13 GB to the device in a 60 s pgbench run
  (121,391 WRITEs, ~107 KB each)
status: Done
assignee:
  - '@main-session'
created_date: '2026-10-23 18:01'
updated_date: '2026-10-23 19:26'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: high
ordinal: 350000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team round 6 (66d56a4): fxdax-nodax wrote 13,014,093,824 bytes to the device per 60 s pgbench -c 4 -j 2 run, 121,391 WRITEs averaging ~107 KB, against 225 MB with DAX; pgbench writes far less. Likely writeback writing whole large folios (TASK-26.19.11's large folios for files not on DAX) for small changes; may be the write amplification behind TASK-91.11 (WRITE time growing run over run) and TASK-91.5.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Bytes the device is written per pgbench run without DAX explained and within a small multiple of what Postgres writes
- [x] #2 WRITE time no longer grows run over run from it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Cause: readahead makes folios up to FXFS_IO_MAX (1 MiB); write_end dirtied the whole folio and writepages wrote it whole, so each 8 KiB Postgres page cost up to 1 MiB. Fix (1586daee): a per-large-folio dirty-page bitmap (iomap_folio_state-like) in folio private; write_end sets the written pages, other dirtying (mmap page_mkwrite) sets all; writepages writes each dirty run as a part of fxfs_write_many; release/invalidate_folio free it, filemap_migrate_folio carries it.
GCE r63 (us-east1-b), five 60 s runs, no DAX: tps 807/818/801/721/750 -> 1264/1281/1267/1271/1234; device bytes per run 7.5-8.8 GB -> 2.0..1.5 GB; device write avg 666-802 us growing -> 367-386 us flat. DAX unchanged. Postgres's own writes (pg_stat_wal + pg_stat_io, printed per run since 506128b8): device is 1.64/1.54/1.82/1.75x of them per run (was ~8x). Guest checks (new partial_writeback) and crash tests pass with and without DAX; upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
No-DAX writeback wrote whole large folios (up to 1 MiB) for each 8 KiB Postgres page. A per-large-folio dirty-page bitmap (as iomap_folio_state) now has writeback write only the dirty pages (1586daee). GCE pgbench without DAX: 807-750 -> ~1260 tps flat; device bytes per run 7.5-8.8 GB -> 1.5-2.0 GB, 1.5-1.8x what Postgres writes (printed per run since 506128b8); device write avg flat at ~375 us. Verified: guest checks (new partial_writeback) and crash tests with and without DAX on GCE; workspace nextest on macOS (scripts/test.sh) and Linux (GCE r63, 2544 passed); upstream-changes.py --check.
<!-- SECTION:FINAL_SUMMARY:END -->
