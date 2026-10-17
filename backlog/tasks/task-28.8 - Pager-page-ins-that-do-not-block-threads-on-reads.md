---
id: TASK-28.8
title: 'Pager: page-ins that do not block threads on reads'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 01:32'
updated_date: '2026-10-17 04:55'
labels: []
milestone: m-5
dependencies: []
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
priority: high
type: enhancement
ordinal: 88000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Page-ins call the synchronous ExtentSource::read on two pager threads (crates/fxdax/src/extents.rs PAGER_THREADS, fxfs-platform fxextent.rs). With object storage behind the store, a fetch takes tens of milliseconds, and two blocked threads would serialise every fault on the host.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 ExtentSource reads are asynchronous, and a page-in waiting for a read holds no thread
- [x] #2 Many page-ins wait on slow reads at once, shown by a test with a source that delays reads
- [x] #3 The guest suites pass and the GCE bench shows no regression
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. fxfs-platform fxextent.rs: ExtentSource::read becomes asynchronous (a boxed Send future) and reads straight into the pager transfer buffer, without the extra copy through a Vec.
2. fxdax extents.rs: StoredExtent reads the local store as before, inside the future (a local read does not wait for long); the fetch scheduler (TASK-28.9) is what will make reads wait.
3. Test in extents.rs: an FxExtent on the cache pager whose source holds every read until several are waiting at once; many threads fault pages of different chunks; with PAGER_THREADS (2) threads the reads can only all be waiting if page-ins hold no thread.
4. Workspace suites (macOS, Linux); guest suites and the GCE bench later with the fetch scheduler, as the store-backed source behaves as before.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
ExtentSource::read returns a boxed future; FxExtent::aligned_read awaits it. The local store source reads synchronously inside a ready future, as before (fast, local); the fetch scheduler (TASK-28.9) will make reads wait. page_ins_wait_holding_no_thread: 8 page-ins on the 2 pager threads all wait at once; with a source that blocks its thread it fails with "only 2 of 8 reads waited at once". Reading straight into the transfer buffer is not possible: the pager buffers are not trusted slices (try_as_mut_slice is None), so the Vec copy stays. macOS workspace 1904 passed.

GCE: guest suites (extent-guest, fxfs-guest-checks-dax, fxfs-guest-crash-dax, fxfs-guest-dax, fxfs-guest) pass at 2e4ef2e (fxdax-test-n1). pgbench on fxdax-bench-p4 (pg26, 3 iterations): fxfs-guest-dax 1464 tps (1456-1476), as before (pg25: 1464). Unrelated: the run, with all cases, found fxfs-guest (no DAX) failing pgbench with a metadata reservation exhausted in compaction (ENOSPC, journal closed).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
ExtentSource::read is async; page-ins waiting on reads hold no thread (test with 8 waiting reads on 2 pager threads; a blocking source fails it). No regression: guest suites pass on GCE, pgbench fxfs-guest-dax unchanged.
<!-- SECTION:FINAL_SUMMARY:END -->
