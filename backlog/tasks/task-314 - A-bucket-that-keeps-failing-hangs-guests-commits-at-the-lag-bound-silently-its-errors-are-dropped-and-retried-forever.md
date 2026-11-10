---
id: TASK-314
title: >-
  A bucket that keeps failing hangs guests' commits at the lag bound, silently:
  its errors are dropped and retried forever
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-10 07:36'
updated_date: '2026-11-10 08:32'
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_remote::store: keep the object_store error's cause; count failed requests by kind (Stats::failures, the stats line's NAME-failed=, metrics fxdax_extentd_object_storage_failures_total); log failures rate-limited per kind of request: the first with its cause, then every 30 s with the count and the latest cause, and once requests of the kind answer again.
2. Outcome, from doc-7 decision 3 (commits stall at the lag bound, as a hard NFS mount hangs): nothing fails and nothing is lost; documented in doc-12 as a failure row: what the guest sees and when.
3. Backoff: already 50 ms doubling to 5 s; shown by the new tests.
4. Tests: a Store over a fault-injecting object store whose writes fail with ENOSPC (logged cause, counts, recovery); a store whose bucket fills: commits stall at the lag bound, failures counted, then the bucket clears and every commit uploads.
5. scripts/test-e2e.sh: prints the bucket's failure lines from extentd.log, failing the run for its own file bucket.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Outcome decided (AC #2). Options: (a) commits wait at the lag bound, nothing failing, the cause logged; (b) commits fail (EIO) after a bound of time or failures. doc-7 decision 3 settles it: at the lag bound commits stall, as a hard NFS mount hangs; doc-12 said nothing of a bucket failing. (a) is also the conservative one: what waits is durable on the host's disk, and failing it would hand the guest's Fxfs fsync errors it shuts down on (or a Linux page cache that dropped the pages), turning a cure-able outage (free the disk, renew credentials) into data loss. Loudness instead: extent_remote::store logs each kind of request's run of failures with the store's own error (first failure, then every 30 s with counts, then when answered again), counts them (stats line failed-NAME, GetStats ObjectStorageStats.failures, metric fxdax_extentd_object_storage_failures_total). doc-12 has the row and what the guest sees and when; doc-7's Uploads points at it.
Backoff: already 50 ms doubling to 5 s (store::backoff), now asserted on a persistently failing create too (25 failures in 95 s, not thousands).
Tests: extent_remote store::tests::writes_failing_say_why_until_they_land (a Full object store: ENOSPC behind object_store's Generic, tokio paused clock: exact log lines and counts, recovery); fxdax remote::tests::commits_wait_at_the_lag_bound_while_the_bucket_is_full (commits stall at max_lag, failures counted, uploader not stopped, then every commit uploads once the bucket has room). AC #3: scripts/test-e2e.sh writes the daemon's failure lines to bucket-failures.txt, prints the first (with df of the bucket) and fails the run when its own tmpfs file bucket failed.
<!-- SECTION:NOTES:END -->
