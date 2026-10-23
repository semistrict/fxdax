---
id: TASK-91.13
title: >-
  Guest virtio-extent driver: spin only as long as a request is expected to take
  (VEXT_SPIN_NS)
status: In Progress
assignee:
  - '@main-session'
created_date: '2026-10-23 18:01'
updated_date: '2026-10-23 19:45'
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
- [ ] #1 The driver spins only for requests whose recent latency is under its spin budget
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
