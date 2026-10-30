---
id: TASK-111.6
title: >-
  Latency distributions for guest fault, seal, evacuation and window MAP, not
  only sums
status: To Do
assignee: []
created_date: '2026-10-28 01:25'
updated_date: '2026-10-30 01:43'
labels: []
dependencies: []
parent_task_id: TASK-111
priority: medium
ordinal: 420000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Guest-side fault, seal, evacuation and MAP timings exist only as totals, so tails and shifts within a run are invisible (TASK-111). extentd already has request histograms.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Guest fault, seal, evacuation and MAP latencies are exported as histograms
- [x] #2 The bench JSON (see the pgbench entry point task) includes their percentiles per run
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
On main (bc1a436df): fs/fxfs/hist.h, log2 buckets of 1.024 us x 2^k, 24; /proc/fs/fxfs NAME_hist for window_map_requests, window_unmaps, dax_faults, dax_evacuate_hold, dax_evacuate_map (C) and device_reads, _writes, _commits, seal_waits, seals, writes_held_by_seals (Rust Timed); pgbench-gate.py's JSON 'latency': each run's count, p50/p90/p99 us from the histograms' differences across it (test test_each_runs_latencies_are_its_guests_histograms_over_it). GCE r261 at f85821427 (release): fxfs-guest-checks, -checks-dax (test_stats checks the histograms grow), -dax, -pgbench-dax passed; pgbench-dax tps 1156.9, 1048.5, 991.0, 972.9, 1004.8; per run seals p50 bucket 524 us, p99 1-2 ms; window MAP requests p50 524 us, p99 1-2 ms; evacuations hold p50 1 ms, p99 2 ms. The faults test's check of the C histograms' 24 buckets runs on the debug kernel's next faults-dax round.
<!-- SECTION:NOTES:END -->
