---
id: TASK-237
title: >-
  An import inside extentd skips the daemon's part cache: its acknowledged
  segments are deleted and its check reads the image back from the bucket (GCE
  r970: 6.8 GB downloaded for 11 GB uploaded)
status: Done
assignee: []
created_date: '2026-11-07 21:29'
updated_date: '2026-11-07 22:19'
labels:
  - perf
dependencies: []
references:
  - docs/measurements/fork-of-a-10-gib-base-2026-11-07.md
priority: medium
type: bug
ordinal: 673000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-165.7's fork timing (GCE r970, main 3bdec3934, docs/measurements/fork-of-a-10-gib-base-2026-11-07.md on land-x4). extentd imported a 10 GiB tar layer (2,560 files of 4 MiB of random bytes) into a 16 GiB store through its control API (`fxdaxctl import`, ImportImage), on a file bucket, with a part cache of its own (`FXDAX_EXTENTD_CACHE`). The daemon uploaded 10.98 GB of segments and downloaded 6.78 GB back (get-range 6,800, get 248), and afterwards its part cache held 780 bytes and the store's directory 26 KB: the base was in the bucket alone. The import took 221 s, against 119 s for the import and 54 s for its uploads on main before the restructure (aee7cad93), which kept the segments in the store and downloaded 40 KB. On GCS, every import would download about the image's size again, at GCS's latency.

Where: extentd's import (`crates/fxdax/src/extentd/stores.rs:649-669`, `crate::import::store` at :664) makes the store through `Storage` (`crates/fxdax/src/import.rs:63-88`, `Storage::create_unmade` at :84), the fxdax CLI's images' path, whose store is a `storage::Kept`:
- `crates/fxdax/src/storage/kept.rs:42` opens the store's uploader with `host_store::keeping()` (`crates/fxdax/src/host_store.rs:126-127`: `upload::Options { token, ..Default::default() }`, so `cache: None`), not extentd's `keeping()` (`crates/fxdax/src/extentd/stores.rs:572-578`, which passes `remote_cache()`, the host's cache). So the segments the bucket acknowledges are deleted, not moved into the daemon's cache (extent_store store.rs `leave`, `Cache::adopt` only with a cache).
- `crates/fxdax/src/storage/kept.rs:63` makes the store's fetcher with `remote::Fetcher::new(bucket.clone(), None)`: no cache, and no read-ahead into one.
- `crates/fxdax/src/import.rs:131`, `storage.check()` after the import, reads the filesystem it made back, every part through that fetcher, a ranged GET each, from the bucket.

A fork of the imported base on the same host then reads it from the bucket too, until its reads fill the cache, where a base made by the daemon's own stores is in the cache once acknowledged (TASK-165.8).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 An import through extentd's control API keeps the image's acknowledged segments in the daemon's part cache (Cache::adopt), and its check reads them there: a test counting the bucket's GETs during an import into a store with a cache, red before
- [x] #2 The fxdax CLI's offline import, where no daemon runs, keeps working without a cache, or with one it is given
- [x] #3 GCE r970's import measured again (scripts/fork-time-gce.sh): downloads well under what it uploads, and the import's time recorded against r970's 221 s
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
- [ ] #4 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #5 scripts/upstream-changes.py --check passes
- [ ] #6 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fixed on land-x5 (6b044d78d): an import inside extentd keeps its image's branch as the daemon's stores keep theirs, with the host's part cache (extentd/stores.rs: crate::import::store_keeping with extentd's keeping(); Storage::create_unmade_keeping; Kept::create and ImageDevice::create take the uploader's options; Kept's fetcher reads through the cache). The fxdax CLI's offline images keep no cache (host_store::keeping, as before). Test: extentd tests imports_read_what_they_made_from_the_hosts_cache, a 24 MiB import through stores::import on a counting bucket with FXDAX_EXTENTD_CACHE set: 6,183 ranged GETs before, 3 after (recovery's reads of segments' footers: one as the store is made, two as the daemon opens it after), 20 of 20 runs; Linux (Lima) fxdax and extent_store 703/703; macOS clippy, workspace 2106/2106, Shuttle.
Measured, GCE r975 (scripts/fork-time-gce.sh, main 3bdec3934 and land-x5 on one host, n2-standard-4 us-east4-a, r970's 10 GiB image): main downloaded 4.63 GB (4,736 ranged GETs) during the import and kept 780 bytes in its cache; land-x5 downloaded 0.7 MB (17 ranged GETs) and kept the image's 10.9 GB in the cache. The import took 205 s against main's 209 s on the same host (r970: 221 s): the download gone, the time not back toward the 173 s (119 s import + 54 s uploads) of main before the restructure. Where it goes: 181 of the import's segment framings logged slow, writing 0.95 s each on average, 172 s in all, framings one queue in closing order since TASK-165.9; not this task's.

Done (land-x5 6b044d78d on main, CI 42417608368 green). AC #1: imports_read_what_they_made_from_the_hosts_cache, red before (6,183 ranged GETs), green after (3, recovery's footers). AC #2: the fxdax CLI's offline import keeps no cache (import::store, Storage::create and create_unmade with host_store::keeping), its tests passing (fxdax import tests, tests/import.rs and tests/fxdaxctl.rs in the macOS workspace 2106/2106). AC #3: GCE r975, above: 0.7 MB downloaded against 4.63 GB, the import 205 s against main's 209 s on one host, the framings' queue its bound. DoD: macOS workspace 2106/2106 and Shuttle; Linux fxdax and extent_store 703/703 (Lima) and CI 42417608368; scripts/upstream-changes.py --check passes on main 6b044d78d; no upstream file changed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
An import inside extentd keeps its image's branch as the daemon's stores do, with the host's part cache: the segments the bucket acknowledges go into the cache and the import's check reads them there. Verified by imports_read_what_they_made_from_the_hosts_cache (6,183 ranged GETs before, 3 after), CI 42417608368, and GCE r975 (0.7 MB downloaded against main's 4.63 GB for a 10 GiB image; its time, 205 s, bound by the framings' queue, not the bucket).
<!-- SECTION:FINAL_SUMMARY:END -->
