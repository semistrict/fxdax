---
id: TASK-164.6
title: Copy-on-write DAX by default
status: To Do
assignee: []
created_date: '2026-11-04 07:58'
updated_date: '2026-11-05 18:59'
labels: []
dependencies: []
parent_task_id: TASK-164
priority: high
type: task
ordinal: 616000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-13 stage 3.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The dax-gate and perf gate pass
- [ ] #2 Writes of created files at no-DAX's tps, base first reads at today's DAX's, the forked pgbench within the gate's threshold
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05, judged on GCE r632 (jailed; docs/measurements/dax-cow-gate.md): not met.
- AC1: the perf gate fails: forked-dax 0.573 against 0.9 (unjailed on the same host 0.570) and decay of dax-density 0.883 against 0.9. The dax-gate wasn't run.
- AC2: writes of created files at no-DAX's tps: met (dax 1460 against 1467 median, 0.995). Base first reads at today's DAX's: not measured against a pre-deletion DAX build; bigmap's cold reads are 5.76x no DAX's (96.8 against 16.8 MiB/s), its guest cache 0. Forked pgbench within the gate's threshold: not met (835 against 1459 median).
- Lead: each dax-forked VMM restricted pages to reads for a KVM worker's write some 2^18 times or more (1,009 log lines: the first thousand, then powers of two), each a failed KVM_RUN and retry for the vCPU; likely the forked gap. Density: DAX saves 14% host memory a VM (2,361 against 2,740 MiB), the VMM holding 700 MiB of the daemon's pages and 790 anonymous.
<!-- SECTION:NOTES:END -->
