---
id: TASK-330.4
title: >-
  Pin-to-pin A/B: the qualification on the previous pin LangSmith runs and on
  the candidate, interleaved, with a report of every metric that moved
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-10 14:00'
updated_date: '2026-11-12 01:45'
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
- [ ] #1 Run once for LangSmith's pin 96c7808b8 vs current main (the user, 2026-11-11: the pin they took, which TASK-310 names), report in docs/measurements; it shows TASK-310's write amplification falling
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
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
<!-- SECTION:NOTES:END -->
