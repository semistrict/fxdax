---
id: TASK-317
title: >-
  extentd's anonymous memory grows to 5-6 GB under an upload backlog, beyond any
  budget
status: To Do
assignee: []
created_date: '2026-11-10 09:19'
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
