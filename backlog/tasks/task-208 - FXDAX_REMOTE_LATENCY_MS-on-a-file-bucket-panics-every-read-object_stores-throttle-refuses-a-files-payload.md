---
id: TASK-208
title: >-
  FXDAX_REMOTE_LATENCY_MS on a file:// bucket panics every read: object_store's
  throttle refuses a file's payload
status: Done
assignee: []
created_date: '2026-11-06 09:37'
updated_date: '2026-11-07 13:02'
labels:
  - bug
dependencies: []
priority: high
ordinal: 646000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by fxfs-guest-soak (TASK-67), GCE r753 round 2 (run FXDAX_SEED in r753's round-2.json, round seed 16104156151717171747): the round's daemons ran with FXDAX_REMOTE_LATENCY_MS=25 against a file bucket, and every store they opened failed with EIO, the VMs exiting 1 at boot; their logs: `thread 'fxdax remote' panicked at object_store-0.14.2/src/throttle.rs:278:41: not implemented`, then `inner future panicked during poll` on every read sharing it. `extent_remote::store::object_store_at` (crates/extent_remote/src/store.rs:234-260) wraps any bucket in `object_store::throttle::ThrottledStore` once the variable is set, and `ThrottledStore::get` calls `unimplemented!()` on a `GetResultPayload::File`, which the file bucket (`extent_remote::local::Local`, TASK-165.1) returns. So bucket latency, the knob doc-12's lag and the sandbox team's 25 ms are measured with, works only on memory and cloud buckets; scripts/test-e2e.sh's own daemon is on a file bucket. Not fixed in TASK-67: extent_remote is TASK-165.9's. A panic in the remote runtime turned into EIO for every store sharing the future, the daemon going on.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Reads through FXDAX_REMOTE_LATENCY_MS from a file:// bucket succeed, late, with a test in the conformance suite for every bucket kind
- [x] #2 fxfs-guest-soak gives file buckets latency again (its Plan::latency_ms no longer zeroed for them)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Evidence: GCE r753, run FXDAX_SEED=17390740708842482561, round 2 (round seed 16104156151717171747; plan: latency 25 ms, daemon killed@131s), extentd-1.log and extentd-2.log, 182 lines each of the panic and EIO. The File payload comes from crates/extent_remote/src/local.rs:145-147, `Local::get_opts` handing `LocalFileSystem::get_opts`'s result on; the panic from object_store-0.14.2/src/throttle.rs:278, `throttle_get`. Replay: FXDAX_WORKSPACE_TESTS=0 FXDAX_E2E_TEST=fxfs-guest-soak FXDAX_E2E_ROUNDS=2 FXDAX_EXTENTD_FAILPOINTS=1 FXDAX_FIRECRACKER_JAIL=1 FXDAX_GCE_MACHINE=n2-standard-8 FXDAX_SEED=17390740708842482561 scripts/test-gce.sh all, with fxfs-guest-soak's latency on file buckets back.

Looking around (2026-11-06): a66f2d996 (TASK-209) has the pgbench cases' daemon add its default 25 ms to buckets in memory or in files alone; on a file bucket that panics here. Its GCE runs kept those daemons' stores in memory (no FXDAX_EXTENTD_REMOTE in the runner), so it never ran on one.

Fixed in 724850a23 (a file bucket's reads are streams, which object_store's throttle takes) and 071be287b (memory_buckets_conform_late, file_buckets_conform_late, s3_buckets_conform_late: the conformance suite under FXDAX_REMOTE_LATENCY_MS, each request at least 5 ms late). land-xk removed the soak's latency zeroing entirely, every bucket getting the plan's latency (TASK-216 for real buckets). GCE r895: fxfs-guest-soak round 2 ran its file bucket at 250 ms latency with the daemon killed at 62 s and passed, 2/2 rounds. Verified (landed as land-xk, 4bf0e45fd): macOS full suite (clippy, workspace 2086/2086, sim, Shuttle, scripts); CI 42363963416 green; GCE r894: workspace 2305/2305, sim 48/48, Shuttle 36/36, default guest suite 15/16 (extent-guest's next_sequence expectation, fixed in 101eeb943); GCE r895: extent-guest 2/2, fxfs-guest-soak 2/2. scripts/upstream-changes.py --check passes on main 0879c8272.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A file bucket's reads are streams, so FXDAX_REMOTE_LATENCY_MS's throttle no longer panics on them; the conformance suite runs late for memory, file and S3 buckets, and the soak gives file buckets latency again. Verified by the conformance tests on macOS and CI, and GCE r895's soak (250 ms on a file bucket, 2/2 rounds).
<!-- SECTION:FINAL_SUMMARY:END -->
