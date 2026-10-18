---
id: TASK-32
title: 'extentd: serve READ requests from the extent cache'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-17 15:26'
updated_date: '2026-10-18 20:21'
labels:
  - performance
  - extentd
dependencies: []
priority: high
ordinal: 101000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Pages of sealed extents the guest maps through DAX come from extentd's extent cache, one copy shared by every VM and fork. The guest Fxfs's READ requests (its metadata: superblock, LSM layer files, journal; and file data without DAX) skip it: ExtentStore::read_stored reads the store with O_DIRECT, or the bucket. So every child forked from a base image reads the base's metadata from disk, or object storage, again at mount and lookup, however many children read it before. Scope: the disk image (extentd's serving of it).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A READ of a sealed extent is served from the extent cache: a second VM or fork reading the same extent reads nothing from the store or the bucket, shown by a test with counters of bytes read
- [x] #2 READs of open extents, and of extents whose data is in object storage only, return the same bytes as before, shown by the extentd and fxdax tests
- [ ] #3 Mount and a cold metadata workload in a fork whose base another VM read first are faster on GCE than before, measured with the fs bench
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. ExtentStore::read reads sealed extents through the extent cache (cached(): the pager-backed VMO, paged in from the store or the bucket), but open extents and followers' reads as before.
2. Docs: extents.rs module docs, read_stored_bytes in stats and the proto.
3. Tests: reads_of_sealed_extents_come_from_the_cache (counters: a fork's read pages in nothing); the daemon fork test's counters.
4. Measure: fork-warm bench case on GCE before and after.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
AC1: reads_of_sealed_extents_come_from_the_cache (a fork's READ of what its base read: 0 page-ins, 0 bytes from the store) and the_daemon_forks_stores_it_serves (one page-in for two windows and a READ, nothing from the store). AC2: the extentd, fxdax and remote tests on macOS and GCE, and 18 guest cases. AC3, measured on GCE (docs/measurements/gce-fs-bench-fork-readcache-before and -after, one c3-standard-8 host, 5 iterations of the warm fork case): bytes READ from the store per fork 4.77 MB -> 4 KiB; the guest's time in READs 146 -> 105 ms; fork-mount 0.37 -> 0.35 s (ranges overlap: 0.35-0.38 and 0.34-0.41); fork-read-usr 0.62 s both. Mount is not clearly faster: AC3 left unchecked for the user's call.
<!-- SECTION:NOTES:END -->
