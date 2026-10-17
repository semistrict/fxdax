---
id: TASK-28.9
title: Fetch scheduler and NVMe cache
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-17 01:32'
updated_date: '2026-10-17 04:34'
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
- [ ] #1 Misses fetch 1 MiB parts of objects; concurrent misses on a part share one GET; demand misses go before read-ahead; GETs in flight are bounded in number and memory
- [ ] #2 Fetched parts are kept in sparse local copies at their real offsets and evicted by punching holes, keeping a free-space floor; unuploaded data is never evicted
- [ ] #3 Every read from NVMe or object storage is checked against the footer CRCs; a mismatch refetches once, then fails with EIO, shown by a test that corrupts both
- [ ] #4 Read-ahead grows along sequential misses and takes neighbouring parts of a segment
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
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
<!-- SECTION:NOTES:END -->
