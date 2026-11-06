---
id: TASK-164.6
title: The copy-on-write DAX perf gate passes
status: To Do
assignee: []
created_date: '2026-11-04 07:58'
updated_date: '2026-11-06 07:05'
labels: []
dependencies: []
parent_task_id: TASK-164
priority: high
type: task
ordinal: 616000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-13's stage 3 was to make copy-on-write DAX the default; since its stage 4 deleted writable DAX (2026-11-04, dead-ends/writable-dax.md), copy-on-write is the only DAX and there is no default to switch: what is left is that its perf gate passes (scripts/perf-gate-gce.sh, docs/measurements/dax-cow-gate.md), single and density phases, on main. The forked pgbench's threshold is the user's decision (2026-11-06): ~0.84 of no DAX is accepted as the cost of doc-13's one host round trip a page's first store (TASK-164.10's r650: 0.836, rerun 0.854), forked-dax at least 0.8; its further cuts are TASK-164.11's, later work.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The perf gate's single and density phases pass on main, jailed, nested (scripts/perf-gate-gce.sh, then its gate phase): forked-dax at least 0.8, dax at least 0.95 (writes of created files at no DAX's tps), the density, decay and bigmap gates at their thresholds
- [ ] #2 The dax-gate passes on main (scripts/dax-gate.sh)
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

2026-11-06, rewritten to what it means now (the user): 'by default' no longer applies, copy-on-write being the only DAX. The old AC 2's 'base first reads at today's DAX's' is dropped: it compared against DAX before writable DAX was deleted, a build no longer kept nor runnable against main's guest and host; bigmap's cold-read gate (bigmap-read, at least 4x no DAX's; r632 5.76, r650 5.13) is what gates first reads now. Its 'writes of created files at no-DAX's tps' is the dax gate (r650 0.999), its forked pgbench forked-dax at 0.8 (r650 0.836). TASK-164.11 stays open, later work.

Decision moved here from OPEN_QUESTIONS.md (2026-11-06): The user's call: copy-on-write DAX's one host round trip a page's first store (doc-13) is worth its cost. Measured after TASK-164.10, jailed, nested (GCE r650): 0.836 (1,217 against 1,455 tps) and 0.854 on a rerun (1,216 against 1,424), from r649's 0.579; doc-13 had expected 0.9-1.0. `scripts/pgbench-gate.py`'s `forked-dax` threshold is 0.8, those numbers with room for a nested host's noise, and docs/measurements/dax-cow-gate.md and doc-13 say so. Left for later, as TASK-164.11 and its two subtasks: the VMM's request path (~165 µs of every request), and a page made private without replacing its mapping (~135 µs a run, a TLB flush of every vCPU); besides, unmeasured, the private pages' pre-fault off the critical path, `private_max` against the density gate, and the window's fragmentation. *Undoing it:* the threshold back to 0.9, which r650 fails.
<!-- SECTION:NOTES:END -->
