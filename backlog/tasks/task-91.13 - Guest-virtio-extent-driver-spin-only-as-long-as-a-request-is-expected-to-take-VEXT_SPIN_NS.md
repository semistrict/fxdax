---
id: TASK-91.13
title: >-
  Guest virtio-extent driver: spin only as long as a request is expected to take
  (VEXT_SPIN_NS)
status: In Progress
assignee:
  - '@main-session'
created_date: '2026-10-23 18:01'
updated_date: '2026-10-24 16:13'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: medium
ordinal: 351000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Round 6: vext_wait spins up to VEXT_SPIN_NS = 100 us before sleeping; with MAPs taking 260-490 us as the guest sees them, the spin burns guest CPU without saving the halt: virtio_extent_call and try_wait_for_completion are 25-47% of guest samples (an interrupt-biased profile). Spin only when the expected reply is under the budget, or adapt per request kind.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The driver spins only for requests whose recent latency is under its spin budget
- [ ] #2 Guest system time in fxfs-guest-pgbench-dax's load and runs lower, tps not lower
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Per-type moving average of request latency (an eighth of the latest, as TCP's SRTT), updated as requests complete. 2. virtio_extent_call spins only if its type's average is under VEXT_SPIN_NS. 3. Verify: guest checks/crash pass; fxfs-guest-pgbench(-dax) tps not lower.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
06dd6a64: per-type moving average; spins only under VEXT_SPIN_NS. GCE: every type now averages ~400 us, so nothing spins (polled 0); no-DAX tps unchanged 1304/1294/1207/1246/1247. AC2's guest system time not measured yet: fxfs-guest-pgbench-profile's run shows the spin gone from the profile (virtio_extent_call / try_wait_for_completion absent from the symbols over 1%).

GCE r65 (2026-10-24, us-east1-b, one host, pgbench-dax, guest cpu ticks from /proc/stat per step): before (kernel prune1, no adaptive spin) load 26.5 s, system 1533 ticks; runs tps 1011/927/896/783/819, system 4416/4490/4652/4553/4609. After (b4) load 20.5 s, system 865; runs tps 998/927/894/825/832, system 4865/4897/4864/4640/4667, user up too and idle down. AC2 holds for the load (-44% system, -23% time), not for the runs: their system time is ~5% higher at equal tps. Interrupt completions 171k a 60 s run, 3 polled. Open: where the runs' system time goes (a perf profile of the guest under pgbench-dax).
<!-- SECTION:NOTES:END -->
