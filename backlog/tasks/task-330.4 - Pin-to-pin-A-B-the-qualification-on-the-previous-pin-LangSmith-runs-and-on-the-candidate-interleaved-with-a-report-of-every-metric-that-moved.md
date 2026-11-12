---
id: TASK-330.4
title: >-
  Pin-to-pin A/B: the qualification on the previous pin LangSmith runs and on
  the candidate, interleaved, with a report of every metric that moved
status: Done
assignee:
  - '@claude'
created_date: '2026-11-10 14:00'
updated_date: '2026-11-12 11:35'
labels:
  - testing
dependencies: []
parent_task_id: TASK-330
priority: medium
ordinal: 733000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
LangSmith bumps pins (6a513509a to 96c7808b8 went bad). Before a pin is offered, run the topology run and the budgets on both, interleaved (scripts/bench-ab.py with fxjob --bench), and publish what changed: disk bytes, memory, latencies, failures. A store-format bump that stops the old kernel reading new stores (Invalid future version 63.0 > 62.0) needs fresh stores per side.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Run once for LangSmith's pin 96c7808b8 vs current main (the user, 2026-11-11: the pin they took, which TASK-310 names), report in docs/measurements; it shows TASK-310's write amplification falling
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. scripts/qualify.sh: the sandboxes' long mode, then fxfs-guest-bucket-soak (through RustFS), then the daemons' budgets as a bench-shaped summary (scripts/qualify-budgets.py).
2. Cases record what they measure and the checks they failed in bench-*.json (common::Measured), each daemon's costs by its kernel's counts (common::ProcessCosts: /proc io, VmHWM, CPU), so a pin's daemon from before its own counters is measured alike.
3. One harness for both sides: test-e2e.sh's FXDAX_E2E_HARNESS (runner, guest agent from the tree running the A/B; extentd, Firecracker, guest kernel from each side).
4. The soak's faults in an HTTP proxy in front of RustFS (faulty_s3.rs), not in the daemon, so both pins meet them; its checks recorded, not stopping at the first.
5. bench-ab-host.sh FXDAX_AB_SUITE=qualify: the sides' NAME=COMMIT, rounds interleaved; bench-ab.py says one-sided metrics, what moved, and each side's failures.
6. One real A/B: 96c7808b8 vs main, 2 rounds, on ubu1; report in docs/measurements.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-11 (claude): built (commits 4cbc08af9, cda3a145d, cfb482c91): scripts/qualify.sh (sandboxes long, then fxfs-guest-bucket-soak through RustFS, then scripts/qualify-budgets.py); cases write bench-CASE.json (common::Measured: measurements and failed checks by kind), each daemon measured by its kernel (common::ProcessCosts: /proc io write_bytes, VmHWM, CPU) so a pin's daemon from before TASK-310's counters and TASK-330.2's budget is measured alike, a missing counter failing its check as such; test-e2e.sh FXDAX_E2E_HARNESS (this tree's runner and guest agent driving another build's extentd, Firecracker and kernel); the soak's faults an HTTP proxy in the case in front of RustFS (faulty_s3.rs: latency, writes 507, every request 503), its checks recorded and the soak running on, 1,200 s at least (object_store retries a 5xx about a minute); bench-ab-host.sh FXDAX_AB_SUITE=qualify, its sides built as work (fxjob) and only its rounds under fxjob --bench (two jobs of one run); bench-ab.py lists one-sided metrics, what moved, each side's failures. The old pin builds under main's scripts (side A built on ubu1, 2026-11-11). Verified: soak on main through the proxy, 1,200 s, passed (failures said 2,712 full / 3,635 out of service, every store uploaded 14-18 s after answering, anon 1,595 of 2,946 MiB, longest fsync 170 s); script tests (macOS); guest part on ubu1 (252, the proxy's own test among them); CI 43107436299 green. The real A/B (96c7808b8 vs main) was started and cancelled at the user's word: it waits for TASK-310 to land, as main's framings are about to change. Filed TASK-367 (clean races a cancel), TASK-368 (the other timed drivers still build in the bench slot).

2026-11-12 (claude): first A/B, 96c7808b8 vs main 3e0d6d1f6 (TASK-310 in), 2 rounds, builds as work: its sandboxes' part ran on NEITHER side: test-e2e.sh's daemon's control socket path, in the host run's nested results, passed SUN_LEN and the daemon exited at once; qualify.sh exited 1 and the A/B went on 4 rounds comparing the soak alone. Fixed: sockets under /tmp (c85e0ee13); a part that left no result is now a failed check, PART.not-run, qualify.sh exit 3, bench-ab.py failing the comparison and the A/B stopping (6c34c4678). Its soak, 1,800 s, through the proxy, both sides 2 rounds: pin's daemon wrote 30.24 GiB to disk vs main's 15.20 (0.50x) for the guests' 15,360 MiB each; peak RSS 3,950 vs 1,628 MiB (0.41x); CPU 243 vs 379 s (main 1.56x); uploads caught up 27.9 vs 24.4 s after answering, longest fsync ~252 s both; the pin failed 'no failure said' for the full and out-of-service phases (TASK-314) and weighs no memory budget (TASK-330.2), every round. Rerunning whole.

2026-11-12 (claude): third A/B (96c7808b8 vs main 09fa51f08): its sandboxes now started on ubu1's long paths (short TMPDIR, ce48681ac); 2 min 41 s into the pin's first round the kernel OOM-killed the pin's host-b extentd in the run's 16 GiB scope (anon 2.97 GB + shmem 2.11 GB, against main's 3.25 GB peak in the same mode: TASK-317's growth in the pin), and systemd stopped the scope, the A/B with it (status 143). Fixed: the sandboxes case stops itself as its cgroup's memory past reclaim (anon + shmem) comes within 1 GiB of the nearest limit, a failed check naming its daemons' memory, its VMs killed, so the round fails and the A/B goes on (a9264a871).

2026-11-12 (claude): fourth A/B (96c7808b8 vs main 0ee1bdb58, run-r3305-20261112T043728Z-8d65a41d). Contaminated: TASK-369's loader (scope fxjob-t369-loader-1808752, ordinary work, 1 GiB rewritten with fsync in a loop on the same /mnt/fxdax-scratch NVMe, ~186 MB/s) ran 01:54:30-02:45:16 EST, beside round 1: pin-r1 (01:54:28-02:27:37) wholly, main-r1's sandboxes part (02:27:37-~02:44) wholly and its soak's first minute. fxjob --bench isolates CPU, cache and memory bandwidth, not the disk. Their framing, durable-lag, command-latency, duration and throughput numbers don't count; pass/fail on memory and correctness stands: the pin's sandboxes stopped by the memory guard (15,365 MiB past reclaim of 16,384, its daemons 4,328 and 4,652 MiB: TASK-317's growth), main's 16 VMs each doing its role with daemons peaking at 3.29/3.24 GB, stores 1.20x the guests' bytes. Round 2 (main then pin) runs after the loader; one more round (pin then main) after it, so the table compares two clean rounds a side, both orders.

2026-11-12 (claude): the fourth A/B's timeline (EST, from each part's resources.log and the journal): pin-r1 01:54:28-02:27:37, main-r1 sandboxes 02:27:37-02:44, soak 02:44-03:14, main-r2 03:14-03:58, pin-r2 03:58-~04:30. TASK-374's drop_caches (~02:50) fell in main-r1's soak, already contaminated by TASK-369's loader. TASK-374's loaders (fxjob-t374-load-2912248 02:56:57-03:07:58, -3120098 03:08:08-04:30:15, -3884249 04:30:47-04:31:34) each waited on its go pipe, which only its bench job (fxjob-t374-bench, started 04:30:13, once this A/B's rounds let the bench slot go) wrote: no load during round 2, which stands clean. Round 2: main's sandboxes failed framings alone (host-a 10 past 1 s, slowest 1,549.6 ms; host-b 12, slowest 4,704.1 ms); the pin's sandboxes stopped by the memory guard again (15,379 of 16,384 MiB past reclaim, daemons 4,326 and 4,839 MiB). A third round (pin then main) runs now, after TASK-374's loader is gone and step 146 caps work's writes on /mnt/fxdax-scratch at 32 MB/s while a bench job runs.

2026-11-12 (claude): the A/B, reported in docs/measurements/ubu1-qualify-ab-2026-11-12 (README.md; ab-clean.md over the clean rounds: run4 round 2, run5 round 1, two a side, both orders). Soak: the pin's daemon wrote 30.25 GiB to disk for the guests' 15,360 MiB (2.0x, TASK-310's double write), main's 15.20 GiB (1.0x); peak RSS 4,359 vs 1,671 MiB; caught up 50.1 vs 21.7 s after the bucket answered; CPU 249 vs 386 s; the pin said no bucket failure (TASK-314). Sandboxes long mode: the pin ran 16 GiB out of memory every round (daemons 4.3-5.6 GB, TASK-317's growth), stopped by the case's guard; main ran its 16 VMs, stores 1.20x the guests' bytes, daemons 3.1 GB, and failed only the framing check (4-12 framings a host past 1 s, slowest 1.2-4.7 s: TASK-310's remainder). run5's main-r1 lost two git VMs to the harness (git's detached auto-gc holding its lock against the role's git gc), fixed (gc.autoDetach=false), guest part green on ubu1. Verification of the code: macOS script tests and fxdax/extent_store suites; ubu1's Linux suite (workspace 2505, sim, shuttle, scripts, guest, firecracker) at cda3a145d and the guest part at each later change; CI green on main (43150361017); upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The pre-pin qualification A/B: bench-ab-host.sh FXDAX_AB_SUITE=qualify runs scripts/qualify.sh (sandboxes long mode, the bucket soak through a fault-injecting S3 proxy in front of RustFS, the daemons' budgets) on LangSmith's pin and a candidate, interleaved, sides built as work and rounds alone under fxjob --bench, one harness driving both builds, each daemon measured by its kernel's counts so a pin without the newer counters is measured alike; bench-ab.py reports each metric, what moved, each side's failed checks, and fails a comparison a part didn't run in. Run for 96c7808b8 vs main on ubu1 (docs/measurements/ubu1-qualify-ab-2026-11-12): TASK-310's write amplification falls from 2.0x to 1.0x the guests' bytes, TASK-317's memory growth and TASK-314's silent bucket failures show on the pin, and main still fails framings past 1 s. Contaminated rounds (a neighbour's disk loader) are marked and left out of the timings.
<!-- SECTION:FINAL_SUMMARY:END -->
