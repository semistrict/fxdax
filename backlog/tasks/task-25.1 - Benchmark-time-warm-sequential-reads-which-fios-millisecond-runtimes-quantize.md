---
id: TASK-25.1
title: >-
  Benchmark: time warm sequential reads, which fio's millisecond runtimes
  quantize
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-10 19:08'
updated_date: '2026-10-10 19:08'
labels: []
dependencies: []
references:
  - docs/measurements/gce-fs-bench-2026-10-10T1823Z.md
parent_task_id: TASK-25
type: chore
ordinal: 26000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Warm sequential reads of the 256 MiB data file take ~40 ms, and fio reports runtimes in whole milliseconds, so ext4's and fxdax's rates come out identical (6,919 MiB/s; docs/measurements/gce-fs-bench-2026-10-10T1823Z.md). Make the warm sequential read a timed run (--time_based), as the warm random reads are, so that it measures a steady rate.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 seq-read-warm runs for a fixed time and reports a rate not quantized by fio's millisecond runtimes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
In test_fs_bench.rs, run seq-read-warm --time_based for RUNTIME, as rand-read-warm is. Verify on GCE.
<!-- SECTION:PLAN:END -->
