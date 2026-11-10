---
id: TASK-314
title: >-
  A bucket that keeps failing hangs guests' commits at the lag bound, silently:
  its errors are dropped and retried forever
status: To Do
assignee: []
created_date: '2026-11-10 07:36'
labels: []
dependencies: []
priority: high
type: bug
ordinal: 722000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen on ubu1 (TASK-311's fxfs-guest-docker-build-dax, FXDAX_DOCKER_BUILD_MIB=1024, run t311-kernel-10-1942): the e2e daemon's file bucket on /dev/shm (14 GB tmpfs) filled after ~14.8 GB of uploads. From then on extentd's object-store counters show up-segments/up-extents frozen while create= climbed ~770 per stats interval (retries), extent_store::upload warned once 'commits wait for object storage: 1195896832 bytes not uploaded, 122155008 past the lag bound of 1073741824', and the guest's fdatasync and writeback waited for good (fxfs: Write I/O request blocked for 10 ... 1060 s; dockerd and a flush-fxfs kworker in D state in fxfs_rt_park) until the runner's 1200 s timeout. Nothing anywhere said ENOSPC: crates/extent_remote/src/store.rs's error() maps every other object_store::Error to Error::Unavailable, dropping its source, and crates/extent_remote/src/bucket.rs's helpers retry Unavailable forever (backoff to 5 s) without logging. In production any persistent bucket failure (quota, credentials revoked, a full disk under a file bucket) would look like a guest hang with no cause in any log.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A request that keeps failing logs its underlying error (rate-limited: the first failure and then periodically, with the count), so the cause shows in extentd.log
- [ ] #2 A test with a bucket whose writes fail persistently (ENOSPC-like) shows the logged cause and the stalled upload, and says what the guest sees (decide: wait with a logged reason, or fail commits after a bound); doc-12 says which
- [ ] #3 The e2e harness's bucket on /dev/shm says when it fills, or the build case keeps within it (TASK-311)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
