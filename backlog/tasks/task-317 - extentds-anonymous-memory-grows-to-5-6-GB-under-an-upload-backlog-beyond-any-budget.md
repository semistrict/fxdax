---
id: TASK-317
title: >-
  extentd's anonymous memory grows to 5-6 GB under an upload backlog, beyond any
  budget
status: Done
assignee:
  - '@claude'
created_date: '2026-11-10 09:19'
updated_date: '2026-11-10 13:19'
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
- [x] #1 What holds extentd's anonymous memory during an upload backlog is measured (heap profile or counters), on one VM writing faster than the bucket takes
- [x] #2 That memory is bounded by a documented budget, or doc-7 says why it need not be, with the bound it actually has
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
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

Fix (commit 'extentd: memory bounded under an upload backlog'): Bucket::put takes bytes::Bytes, so create/update's retries share one copy; extentd sets glibc's M_MMAP_THRESHOLD to 1 MiB at start (extentd::memory::return_large_allocations); FXDAX_EXTENTD_UPLOAD_MEMORY configures the host upload Budget (512 MiB default, >= 64 MiB); Budget::held gives held/peak; stats line 'uploads: held= max-held= budget='; metrics fxdax_extentd_upload_held_bytes, _held_max_bytes, _memory_bytes. Tests: remote::tests::uploads_a_bucket_takes_none_of_hold_no_more_than_their_budget (counted heap; 1 GiB behind; budget 256 MiB; fixed 256 MiB peak, a per-try copy mutant 448 MiB and fails), extentd::memory::tests::large_buffers_freed_go_back (glibc; 1024 MiB resident without the threshold, fails; <16 MiB with). crate::heap: the counting allocator moved out of hostile.rs for every fxdax test.
Proof on ubu1, same workload (docker build, 3 x 512 MiB, MEMORY_BYTES 2 GiB, file bucket on disk, 300 ms latency), extentd alone in a scope:
- before, main 604578a1-era artifacts (ff6a9dc38), no profiler: anon peak 5,902 MB, shmem 1,565 MB, scope memory.peak 9.36 GB (page cache in it), 505 s, 8 stalls at the lag bound;
- after, scope MemoryMax 3.5 GiB: anon peak 718 MB, shmem 1,587 MB, 353 s, 9 stalls, uploads max-held 507 MiB;
- after, scope MemoryMax 3 GiB (2 GiB extent cache + 1 GiB): passed in 538 s, not killed (no 'Memory cgroup out of memory'), anon peak 666 MB, shmem 1,595 MB, 10 stalls, uploads max-held 510 MiB of 512 MiB.
Tests: macOS scripts/test.sh (clippy, workspace 2211, sim, shuttle, scripts) passes. Linux (ubu1, load ~30): 2460 of 2465 pass; durable_commits_keep_the_open_segments_pages_cached_on_the_host and imports_read_what_they_made_from_the_hosts_cache fail on origin/main too (TASK-315, TASK-316: tmpfs /tmp); a_compromised_vm_process_harms_no_tenant_beside_it, page_ins_in_flight_are_bounded and counting_followers_walks_no_extents failed under the load and pass alone on the branch. scripts/upstream-changes.py --check passes.
Found and filed: TASK-325 (framing holds 2-4 segments' worth per store), TASK-326 (stall warning says 0 commits), TASK-327 (read_whole copies each upload once more).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
extentd's memory under an upload backlog was its 512 MiB upload budget held twice (bucket::create cloned each PUT's bytes per try) plus ~2.6 GB of freed segment buffers kept in 134 glibc per-thread arenas. Fixed: Bucket::put takes Bytes (one copy, however many tries); extentd fixes glibc's mmap threshold at 1 MiB so large frees go back; the upload budget is configuration (FXDAX_EXTENTD_UPLOAD_MEMORY), in the stats line and metrics; doc-7 (Uploads, Memory), doc-12 and the README say the bound. Verified by a counted-heap test with a bucket that never answers (fails on a per-try copy), a glibc retention test (fails without the threshold), and on ubu1: the same docker-build backlog went from 5.9 GB anon to 666-718 MB, passing unkilled with extentd capped at 3 GiB including its 2 GiB extent cache.
<!-- SECTION:FINAL_SUMMARY:END -->
