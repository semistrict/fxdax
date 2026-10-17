---
id: TASK-28.9
title: Fetch scheduler and NVMe cache
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 01:32'
updated_date: '2026-10-17 05:50'
labels: []
milestone: m-5
dependencies:
  - TASK-28.7
  - TASK-28.8
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
type: feature
ordinal: 89000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-7 Reading: misses go to object storage through one scheduler per host, and the NVMe cache is the local store own files, sparse. object_store get_ranges fans out only 10 ways, fixed; one connection gives roughly 80-100 MB/s, so a host needs tens of GETs in flight.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Misses fetch 1 MiB parts of objects; concurrent misses on a part share one GET; demand misses go before read-ahead; GETs in flight are bounded in number and memory
- [x] #2 Fetched parts are kept in sparse local copies at their real offsets and evicted by punching holes, keeping a free-space floor; unuploaded data is never evicted
- [x] #3 Every read from NVMe or object storage is checked against the footer CRCs; a mismatch refetches once, then fails with EIO, shown by a test that corrupts both
- [x] #4 Read-ahead grows along sequential misses and takes neighbouring parts of a segment
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Local stores hold extents whose data is in object storage (extent_store Data::Remote, Store::adopt, RemoteData read error, Change::Adopt in the index): a branch taken by an empty store adopts its extents instead of copying them, its state stays whole, frees of them are journaled and uploaded like others.
2. extent_remote::read_range: one ranged GET of the bytes wanted, following moves.
3. fxdax: the remote owns one multi-thread tokio runtime for uploads and fetches; Fetcher reads an adopted extent range from the writer locations; READ blocks on it, the pager source awaits it (holding no thread, TASK-28.8).
4. Next: a fetch scheduler (1 MiB parts, single-flight, demand before read-ahead, a byte budget), the NVMe cache (sparse local copies), read-ahead, integrity CRCs per part; measured on GCE.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
First increment (fetch on demand): tests maps_extents_whose_data_is_in_object_storage (window faults page in pages fetched from the bucket), opens_branches_elsewhere (reads of a taken branch fetch), adopted_extents_are_held_without_their_data (extent_store). Not yet: parts, single-flight, priorities, cache, read-ahead, CRCs.

Second increment (f3ba8fd, cca4b8c, db25805, 5ae5539):
- Objects carry a CRC32C per 1 MiB part of their data plus a 16-byte trailer (layout::with_footer / Frame). This deviates from doc-7's per-extent CRCs per 64 KiB; doc-7 records why.
- extent_remote::fetch: 1 MiB parts; single-flight GETs; a host budget (64 GETs, 64 MiB) with demand before read-ahead and promotion; refetch once on a CRC mismatch, then Corrupt (EIO); abandoned GETs give up their place.
- extent_remote::cache: sparse per-object files, LRU eviction by punching holes, a capacity and a free-space floor. It holds only fetched data, so nothing unuploaded is ever evicted. Cached reads are CRC-checked; a failing part is dropped and fetched.
- Read-ahead doubles along sequential reads, up to 16 parts. It is by object part, so it takes neighbouring extents of a segment.
- fxdax: remote Options.cache; extentd FXDAX_EXTENTD_CACHE (4 GiB floor).
- Deviation: the cache is its own sparse files, not the local store's segment files, which become remote-format objects only with TASK-28.3.
- Tests:
  - layout::tests::objects_hold_their_parts_crcs
  - fetch::tests: 9, including parts_kept_failing_their_crcs_are_fetched (corrupts both copies), demand_goes_before_read_ahead_within_the_budget, read_ahead_grows_along_sequential_reads
  - cache::tests: 4, including evicted_parts_free_their_blocks on Linux
  - remote::tests::reads_keep_what_they_fetch
- Workspace: macOS 1932, Linux 1942. upstream-changes.py --check passes.
- Not done: measuring read throughput on GCE (TASK-28.11, needs a bucket).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Reads of extents whose data is in object storage are now done by 1 MiB parts, each checked against its CRC (carried in every object's trailer), under a shared host budget. Concurrent reads of a part share one GET, demand goes before read-ahead, and a part is refetched once on a mismatch, then the read fails with EIO. Fetched parts are kept in a sparse, LRU, hole-punching cache with a free-space floor, and read-ahead grows along sequential reads. Verified by the fetch, cache, layout and remote tests listed in the notes and by the macOS and Linux workspace suites.
<!-- SECTION:FINAL_SUMMARY:END -->
