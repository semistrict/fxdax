---
id: TASK-317
title: >-
  extentd's anonymous memory grows to 5-6 GB under an upload backlog, beyond any
  budget
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-10 09:19'
updated_date: '2026-11-10 12:04'
labels: []
dependencies: []
priority: medium
type: bug
ordinal: 723000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On ubu1 (TASK-311's fxfs-guest-docker-build-dax, one 8-vCPU VM, 512 MiB of files a build, the daemon started with MEMORY_BYTES=2 GiB, file bucket on disk) the host's global OOM killer took fxdax-extentd three times, each time with anon-rss far past its budget: 5,231,012 kB (02:52, MEMORY_BYTES 8 GiB, bucket on /dev/shm), 6,145,104 kB (04:18, MEMORY_BYTES 2 GiB) and 2,651,596 kB anon + 762,988 kB shmem (03:35). MEMORY_BYTES (crates/fxdax/src/bin/fxdax-extentd.rs) bounds only the sealed extents it caches (shmem); nothing bounds the rest. These runs had uploads behind (extent_store::upload warned of commits waiting at the 1 GiB lag bound), so the likely holder is upload payloads in memory (crates/extent_remote/src/store.rs put() takes the bytes as a Vec<u8>; IN_FLIGHT uploads in crates/extent_store/src/upload.rs), but that is not measured. On a production host a daemon whose memory grows with its backlog is the first thing an OOM killer takes, stopping every VM it serves. Found while working TASK-311; the host was overloaded by other agents, so the growth's driver needs measuring alone.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 What holds extentd's anonymous memory during an upload backlog is measured (heap profile or counters), on one VM writing faster than the bucket takes
- [ ] #2 That memory is bounded by a documented budget, or doc-7 says why it need not be, with the bound it actually has
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Measure on ubu1: fxfs-guest-docker-build-dax (8 vCPUs, 3 builds x 512 MiB, MEMORY_BYTES 2 GiB, file bucket on disk, FXDAX_REMOTE_LATENCY_MS=300), extentd in a systemd scope of its own (test-e2e.sh FXDAX_EXTENTD_MEMORY_MAX) under heaptrack (FXDAX_EXTENTD_WRAP); /proc smaps of the live daemon.
2. Halve upload memory: Bucket::put takes Bytes, so bucket::create/update's retries share the payload rather than clone it per attempt.
3. Give back large frees: mallopt(M_MMAP_THRESHOLD, 1 MiB) at extentd start, so glibc's dynamic threshold and per-thread arenas stop retaining freed 32 MiB segment buffers.
4. Budget: FXDAX_EXTENTD_UPLOAD_MEMORY (default 512 MiB, >= 64 MiB) sets the host's upload Budget; Budget::held reports held/peak; stats line 'uploads: held max-held budget'; metrics fxdax_extentd_upload_{held,held_max,memory}_bytes.
5. Tests: remote's uploads_a_bucket_takes_none_of_hold_no_more_than_their_budget (counted heap, crate::heap moved out of hostile.rs, bucket holding every segment PUT, 1 GiB behind); extentd memory's large_buffers_freed_go_back (Linux, glibc).
6. Docs: doc-7 Uploads, doc-12, extentd's docs.
7. Prove on ubu1: same workload with the fix, extentd scope MemoryMax just above the budget, not killed; peak RSS before/after.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Measured (AC #1), ubu1, main ff6a9dc38, fxfs-guest-docker-build-dax (8 vCPUs, 3 builds x 512 MiB), extentd MEMORY_BYTES 2 GiB, file bucket on /mnt/fxdax-scratch with FXDAX_REMOTE_LATENCY_MS=300, extentd alone in a systemd scope (test-e2e.sh's new FXDAX_EXTENTD_MEMORY_MAX) under heaptrack (FXDAX_EXTENTD_WRAP). Passed in 534 s; commits stalled at the 1 GiB lag bound 8 times. extentd's cgroup: anon rose to 3.0 GB at the first stall and 4.4 GB later, shmem (the extent cache) 0.8-1.5 GB; memory.peak 8.9 GB with page cache.
Heap at its peak, 1.31 GB (heaptrack):
- 507 MB upload bodies read from the WAL (extent_store::framed::read_whole, Writer::create within the 512 MiB upload budget);
- 507 MB the same bytes again: extent_remote::bucket::create cloned its Vec for each PUT attempt (bucket.put(key, bytes.clone(), ..)), so the budget held twice what it counts;
- 108 MB + 100 MB framing (extent_frame::framed's output growing, Aligned::new reading the raw segment): one framer per store, ~2-4x a segment (TASK-325);
- 68 MB fetched parts (within the 64 MiB fetch budget);
- the rest small (page tables of windows, cache entries, < 10 MB).
Anon beyond the heap: /proc/PID/smaps of the live daemon (RssAnon 2.77 GB) found 134 glibc per-thread arena heaps (64 MiB-aligned, up to 64 MiB each) holding 2.63 GB, against a live heap ~1.3 GB: glibc raises its mmap threshold to the largest buffer freed (32 MiB segments) and keeps freed segment-sized buffers in its arenas, one per thread up to 8x CPUs. So: ~half the anon was the upload budget held twice, ~half allocator retention, growing with how many threads had touched a segment buffer. Nothing grew with the backlog's length itself: the WAL is on disk, and gather takes at most IN_FLIGHT segments.
<!-- SECTION:NOTES:END -->
