---
id: TASK-92.21
title: >-
  Read-ahead decompresses each part and drops it, then the demand read re-reads,
  re-checks and re-decompresses it, under the host-wide cache lock
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-22 21:06'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 296000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Parts read ahead are checked, unpacked and copied into an Arc, then dropped (fetch.rs:325-334, 405-407). The demand read then preads them back from NVMe and redoes the CRC and zstd (cache.rs:82-83; fetch.rs:427). This runs on the shared 2-8 thread runtime (remote.rs:241-249), and Cache::put holds the host-wide mutex across file I/O and evictions (cache.rs:105-124, 152-163). Reading 2 GiB of a branch from GCS costs 1-2 s of extra CPU and 2 GiB of extra NVMe reads.

Where: crates/extent_remote/src/fetch.rs:325-334, crates/extent_remote/src/fetch.rs:405-407, crates/extent_remote/src/fetch.rs:427, crates/extent_remote/src/cache.rs:82-83, crates/extent_remote/src/cache.rs:105-124, crates/extent_remote/src/cache.rs:152-163, crates/fxdax/src/remote.rs:241-249.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present after 7f590fe (the reader module removed; one fetcher path). read_ahead spawns this.part(.., Priority::Ahead), and its results are joined and dropped (crates/extent_remote/src/fetch.rs:346-353). Fetching::part unpacks every fetched part (fetch.rs:403-428). The demand read then goes kept(): cache.get preads the stored bytes back (cache.rs:74-90) and tail.unpack redoes the CRC and zstd (fetch.rs:435-448). Cache::put holds the host-wide kept mutex across statvfs, open, set_len, write_all_at and evict/punch (cache.rs:95-130). Everything runs on the remotes' 2-8 thread runtime (crates/fxdax/src/remote.rs:262-270). Not tracked. Related: TASK-28.9, TASK-38, TASK-29.3.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [x] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A fetcher now keeps fetched parts as checked stored bytes and unpacks each one once, for the read that wants it, so read-ahead never decompresses and a demand read takes what was read ahead instead of re-reading the cache. The cache's lock now covers only its bookkeeping, not file I/O. Cache file I/O runs in block_in_place, so it no longer stalls the shared remotes runtime, and what fetchers keep in memory is shared across the host within one 64 MiB bound. Before and after measurements are in 876b7563, 1b0fe472, 525e6f9e and f50e3cd9: 64 KiB sequential reads went from 1.53 s to 134 ms, and the slowest cache read while parts are kept went from 14-97 ms to under 0.5 ms. The macOS and Linux (GCE) suites pass.
<!-- SECTION:FINAL_SUMMARY:END -->
