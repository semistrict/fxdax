---
id: TASK-237
title: >-
  An import inside extentd skips the daemon's part cache: its acknowledged
  segments are deleted and its check reads the image back from the bucket (GCE
  r970: 6.8 GB downloaded for 11 GB uploaded)
status: To Do
assignee: []
created_date: '2026-11-07 21:29'
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
- [ ] #1 An import through extentd's control API keeps the image's acknowledged segments in the daemon's part cache (Cache::adopt), and its check reads them there: a test counting the bucket's GETs during an import into a store with a cache, red before
- [ ] #2 The fxdax CLI's offline import, where no daemon runs, keeps working without a cache, or with one it is given
- [ ] #3 GCE r970's import measured again (scripts/fork-time-gce.sh): downloads well under what it uploads, and the import's time recorded against r970's 221 s
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
- [ ] #4 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #5 scripts/upstream-changes.py --check passes
- [ ] #6 Deviations from upstream documented where they occur
<!-- DOD:END -->
