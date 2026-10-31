---
id: TASK-146
title: >-
  Sandbox team, round 9 (2026-10-31 report): DAX at half of no DAX under
  pgbench; synchronous window round trips on write(2)
status: To Do
assignee: []
created_date: '2026-10-31 22:15'
labels:
  - sandbox
  - performance
dependencies: []
priority: high
ordinal: 496000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Measured at 5151882, which has none of the work since cc4f61cf0. Overlay/no DAX/DAX tps: default 3366/1736/1012, synchronous_commit=off 4793/4340/2436, fsync=off 4856/4481/2230, select-only equal. Correction to the report's framing: fxfs_device_submit, its 44 us 'other device requests', is asynchronous, submission alone. Round trips from the guest's counters (GCE r353, jailed, GCS): reads 472-545 us, writes 338-597 us, commits 290-459 us, a MAP request 373 us. So a MAP is an ordinary round trip, and the transport's 300+ us is what to explain; DAX pays it inside write(2) where no DAX defers it to writeback. Polling never runs: its gate is recent requests under 100 us: polled 15 of 1,043,055.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
