---
id: TASK-209
title: >-
  fxfs-guest-pgbench-dax-forked's daemon adds 25 ms to a real bucket's every
  request: its crash restart passes pg_ctl's 900 s on GCS (GCE r740, r743)
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 10:30'
updated_date: '2026-11-06 10:31'
labels:
  - real-bucket
  - guest-tests
dependencies: []
priority: high
type: bug
ordinal: 646000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The pgbench cases run a daemon of their own (test_fxfs_guest_pgbench.rs, `run_stores`) whose bucket answers 25 ms late, FXDAX_REMOTE_LATENCY_MS unless set, so that a bucket in its memory is as far as the sandbox team's RustFS was. With FXDAX_EXTENTD_REMOTE naming a real bucket (scripts/test-gce.sh with FXDAX_GCE_STORAGE), that daemon keeps its stores in it, and the 25 ms went on top of the real latency. fxfs-guest-pgbench-dax-forked then failed on GCS twice (r740 us-east1-c, r743 us-central1-a): after `pg_ctl -m immediate stop`, the restart that recovers from the WAL, its reads of what the fork inherited faults on the DAX window paged in by ranged GETs one after another, did not finish within pg_ctl's `-t 900`. Faults averaged 5.9 ms and 7.6 ms; on a file bucket 0.40 ms.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The case's daemon adds 25 ms to a bucket in its memory or in files and nothing to a real one, unless FXDAX_REMOTE_LATENCY_MS says otherwise (a unit test, red before)
- [x] #2 fxfs-guest-pgbench-dax-forked passes on a GCS bucket, jailed with the production seccomp filter
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. A function of the given latency and the bucket's URL, its test written against the old behaviour first (red). 2. 25 ms to a bucket in memory or files alone. 3. The harness's tests in Linux Docker; the case on GCS, jailed.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
How found: the first guest suites against a real bucket (GCS, this task's runs). r740 (us-east1-c) and r743 (us-central1-a) failed alike: "OK checked" missing, pg_ctl start -t 900 after the immediate stop "server did not start in time"; the daemon's faults 5.9 ms and 7.6 ms on average against 0.40 ms in r741 (file bucket, where the case's daemon kept its stores in memory, 25 ms away), its GETs 16,924 and 12,737 ranges. The latency is added by object_store_at (crates/extent_remote/src/store.rs:247) to any bucket, the real one too.

Blind spot: the case had only ever run with its daemon's bucket in memory (spawn_daemon names memory:///NAME unless FXDAX_EXTENTD_REMOTE is set, and scripts/test-e2e.sh sets it for its own daemon alone). A knob standing in for a far bucket was never combined with a far bucket.

Test: test_fxfs_guest_pgbench.rs host::tests::latency_is_added_to_local_buckets_alone; red with the old default (gs:// got "25"), green after. guest/tests' test_cases suites in Linux Docker (rust:1, arm64): host 66/66, guest 66/66 (the 3 sqlite tests need sqlite3 in the container, then pass). GCE r744 (us-east1-c, jailed, seccomp real, gs://echophase-fxdax-bench): fxfs-guest-pgbench-dax-forked passes, the restart 97.9 s, faults 0.59 ms on average.

Looking around: no other case adds latency of its own; scripts/soak-gce.sh exports FXDAX_REMOTE_LATENCY_MS=25 for every daemon of a soak, which would add it to a real bucket too, but no soak runs with one (its host has no storage scope). guest checks' remote scale (test_fxfs_guest_checks/host.rs:83-84) treats every named bucket as remote, which only real buckets are named for in runs: consistent.

Structure: the class (latency added to what is already far) would be impossible if object_store_at added FXDAX_REMOTE_LATENCY_MS to memory:// and file:// buckets alone; it is in crates/extent_remote, which TASK-165.9 is rewriting, so left there, noted for it. upstream-changes.py --check passes. Workspace not run: no crate of it changed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The pgbench cases' daemon adds its default 25 ms only to a bucket in memory or in files, not to a real one, unless FXDAX_REMOTE_LATENCY_MS says otherwise (added_latency). Red-then-green unit test; guest/tests' suites 66/66 per feature in Linux Docker; GCE r744 on GCS, jailed: fxfs-guest-pgbench-dax-forked passes (1548 s of 1800; restart 97.9 s, which took more than 900 s in r740 and r743). Its balance sum after the restart still takes 927 s, on a file bucket 689 s: TASK-165.12.
<!-- SECTION:FINAL_SUMMARY:END -->
