---
id: TASK-208
title: >-
  FXDAX_REMOTE_LATENCY_MS on a file:// bucket panics every read: object_store's
  throttle refuses a file's payload
status: To Do
assignee: []
created_date: '2026-11-06 09:37'
updated_date: '2026-11-06 12:53'
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
- [ ] #1 Reads through FXDAX_REMOTE_LATENCY_MS from a file:// bucket succeed, late, with a test in the conformance suite for every bucket kind
- [ ] #2 fxfs-guest-soak gives file buckets latency again (its Plan::latency_ms no longer zeroed for them)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Evidence: GCE r753, run FXDAX_SEED=17390740708842482561, round 2 (round seed 16104156151717171747; plan: latency 25 ms, daemon killed@131s), extentd-1.log and extentd-2.log, 182 lines each of the panic and EIO. The File payload comes from crates/extent_remote/src/local.rs:145-147, `Local::get_opts` handing `LocalFileSystem::get_opts`'s result on; the panic from object_store-0.14.2/src/throttle.rs:278, `throttle_get`. Replay: FXDAX_WORKSPACE_TESTS=0 FXDAX_E2E_TEST=fxfs-guest-soak FXDAX_E2E_ROUNDS=2 FXDAX_EXTENTD_FAILPOINTS=1 FXDAX_FIRECRACKER_JAIL=1 FXDAX_GCE_MACHINE=n2-standard-8 FXDAX_SEED=17390740708842482561 scripts/test-gce.sh all, with fxfs-guest-soak's latency on file buckets back.

Looking around (2026-11-06): a66f2d996 (TASK-209) has the pgbench cases' daemon add its default 25 ms to buckets in memory or in files alone; on a file bucket that panics here. Its GCE runs kept those daemons' stores in memory (no FXDAX_EXTENTD_REMOTE in the runner), so it never ran on one.
<!-- SECTION:NOTES:END -->
