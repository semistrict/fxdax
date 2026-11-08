---
id: TASK-131
title: >-
  Sandbox team, round 8 (2026-10-29 requests): DAX regression at b5d3246, fsync
  without seals, cheaper first reads of a shared base, refusals' cost, a CI
  performance gate, observability
status: Done
assignee: []
created_date: '2026-10-29 21:13'
updated_date: '2026-11-08 04:19'
labels:
  - sandbox
  - performance
dependencies: []
priority: medium
ordinal: 471000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Their report: DAX sharing works for them (each added VM ~285 MB of host memory against ~640 MB on their runtime; ~350x fewer object-storage requests than JuiceFS over a pgbench VM's life). Asks: performance (DAX 14% slower at b5d3246, 720 against 839 tps, and losing ~21% over five runs: profile it, their guess the seal correctness work; whether every fsync needs a seal, DAX seals 0.54 ms against 0.32 without, almost all ~3 window remaps; first reads of a shared base at ~130 MB/s with every page in memory; ~4,000 refused guest writes per pgbench VM: their cost; bare-metal and nested numbers apart); testing (a CI performance benchmark, one entry point, five-run DAX/no-DAX pgbench and a density test, machine-readable, failing on run 5 against run 1, DAX against no DAX, and one long-lived extentd serving many VMs; measured against bench-fs-ext4-dax-pmem); observability (gauges: the extent cache's pinned and idle entries, the window's live mappings; the VMM's cost per window change in /metrics; latency histograms for guest faults, seals, MAPs (TASK-111.6); tracepoints at seal, MAP, evacuation, reclaim (TASK-44.16); a build-commit stamp on every component (TASK-111.7); guest kernel symbols that survive KASLR). Related: TASK-111, TASK-112 and their subtasks; TASK-112.3 answered 'does every fsync need a seal' for round 7b.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-06 tidy: priority high to medium: the sandbox team is happy with performance and integrating (the user, 2026-11-03, TASK-146.1); base first reads against today's DAX are TASK-164.6's gate meanwhile.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Round 8's asks are answered, each subtask Done:
- DAX regression at b5d3246: profiled and bisected (TASK-131.1);
- fsync without a seal (TASK-131.2);
- first reads of a shared base, superseded: about 270 MB/s now, not ~130, with whole-extent maps (TASK-131.3, r941);
- refused writes' cost (TASK-131.4);
- the CI performance gate (TASK-131.5): scripts/perf-gate-gce.sh, docs/measurements/dax-cow-gate.md, r903 passing every gate;
- observability (TASK-131.6);
- crash-dax-killed's lost rewrite (TASK-131.7).
Its other asks went to TASK-111 and TASK-112, which hold them.
<!-- SECTION:FINAL_SUMMARY:END -->
