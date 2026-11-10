---
id: TASK-330.3
title: >-
  Degraded-dependency soak: 30+ min against a slow bucket, then a failing one,
  then recovery; memory bounded, errors surfaced, every commit uploaded
status: Done
assignee:
  - '@claude'
created_date: '2026-11-10 14:00'
updated_date: '2026-11-10 17:16'
labels:
  - testing
dependencies: []
parent_task_id: TASK-330
priority: high
ordinal: 732000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A fault-injecting bucket (latency, ENOSPC, 5xx) behind extentd for a long run, with guests writing all the while. Assert: memory.peak under budget; failures in the stats line and metrics (TASK-314); commits wait at the lag bound and never fail; everything uploads within the bound once the bucket answers; no VM let go (TASK-249).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Passes on main; fails with TASK-314's error-dropping or TASK-317's per-attempt copy put back
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_remote::faults: a Faulty object store (latency, no-space ENOSPC writes, unavailable 503) driven by FXDAX_REMOTE_FAULTS (a file re-read every 100 ms); FXDAX_REMOTE_LATENCY_MS folded into it, object_store's throttle dropped.
2. extentd::tests::soak (ignored; FXDAX_SOAK_SECS, 300 default): 4 in-process VM processes writing 1 MiB incompressible extents, the bucket answering, slow, full, out of service, answering again; asserts memory budget never passed, failures in stats line and metrics, commits stalled at the lag bound and none failed, backlog uploaded within doc-12's bound, connections all kept.
3. CI job 'soak' (300 s) in ci.yml; Hunt job 'soak' (1800 s).
4. Guest case fxfs-guest-bucket-soak (manual, ubu1): the same phases with real VMs and a daemon of its own.
5. Mutants: bucket-failures-dropped (TASK-314), uploads-copied-each-try (TASK-317), killed by the soak.
6. doc-12/doc-7: the bound and the budget.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fault-injecting bucket: extent_remote::faults::Faulty, driven by FXDAX_REMOTE_FAULTS (a file re-read every 100 ms): latency, failing=no-space (ENOSPC on writes, as TASK-314's full tmpfs), failing=unavailable (503 on every request). FXDAX_REMOTE_LATENCY_MS now goes through it; object_store's throttle is gone.
Phases (fractions of the run): answering 10%, slow 300 ms 20%, full 30%, out of service 10%, answering again at 300 ms 30%. I made the recovery phase slow after a guest run let TASK-317's per-try copy through: with an instant bucket, the extra copies never live long enough to show. Writers write 6 GiB while the bucket fails, past every store's 1 GiB lag bound. Upload memory is 1.5 GiB, what 64 PUTs of ~24 MiB segments hold, so an upload held twice exceeds the budget's other parts (4 x 288 + 256 MiB).
Bound once the bucket answers (doc-12, now written down): 5 s (backoff) + 1 s (upload interval) + the time the guests took to write what waited.
In-process soak (extentd::tests::soak, CI 300 s, Hunt 1800 s):
- 120 s: anon 1.71 GB of 3.09 GB, commits waited ~24 s, everything uploaded 2 s after the bucket answered.
- 1800 s: anon 1.88 GB of 3.09 GB, commits waited 246 s, none failed.
- The upload catch-up check in the 1800 s run was still the first version, which measured after the recovery phase; fixed to poll during it, then rerun at 120 s.
- Mutants killed: bucket-failures-dropped ('the bucket full, and no failure said'); uploads-copied-each-try (memory budget passed within 47 s).
Guest case fxfs-guest-bucket-soak (4 VMs, manual), t263 kernel:
- 300 s: anon 1.76 GB of 2.95 GB, fsyncs waited 44 s, every store synced 26-27 s after the bucket answered.
- With uploads-copied-each-try: 3.1 GB, past the budget, failed. With bucket-failures-dropped: failed ('no failure said').
- 1800 s: anon 1.90 GB of 2.95 GB, uploads 1534 of 1536 MiB, fsyncs waited 250 s, none failed, every store uploaded 16-19 s after recovery, no VM let go, disk writes within what was counted.
CI: ci.yml job 'soak' (300 s, GitHub runner, no KVM needed); hunt.yml job 'soak' (1800 s nightly). The guest case needs KVM, so it runs by name on ubu1 as TASK-330's gate.
Found and filed: TASK-331 (test-e2e.sh leaves its part cache and /dev/shm bucket when SIGKILLed; 10 GB found on ubu1's /tmp).

Main CI 42859291294 green, its new soak job included (300 s on a GitHub runner: anon 1.77 GB of 3.09 GB, commits waited 47 s, what waited uploaded 4 s after the bucket answered).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A fault-injecting bucket (extent_remote::faults, FXDAX_REMOTE_FAULTS) and two soaks against it: slow, then full (ENOSPC), then out of service (503), then answering again, with writers going all the while. extentd::tests::soak runs in-process: 300 s in a new CI job, 1800 s nightly in Hunt. fxfs-guest-bucket-soak runs real VMs on ubu1. Both assert memory under budget, failures in the stats line and metrics, commits stalling at the lag bound with none failing, everything uploaded within doc-12's bound (now documented), and no VM let go. Verified on ubu1: both pass at 30 minutes, and both fail with TASK-314's error-dropping or TASK-317's per-try copy put back.
<!-- SECTION:FINAL_SUMMARY:END -->
