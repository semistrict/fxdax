---
id: TASK-164.6
title: The copy-on-write DAX perf gate passes
status: Done
assignee: []
created_date: '2026-11-04 07:58'
updated_date: '2026-11-06 18:51'
labels: []
dependencies:
  - TASK-164.12
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
- [x] #1 The perf gate's single and density phases pass on main, jailed, nested (scripts/perf-gate-gce.sh, then its gate phase): forked-dax at least 0.8, dax at least 0.95 (writes of created files at no DAX's tps), the density, decay and bigmap gates at their thresholds
- [x] #2 The dax-gate passes on main (scripts/dax-gate.sh)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05, judged on GCE r632 (jailed; docs/measurements/dax-cow-gate.md): not met.
- AC1: the perf gate fails: forked-dax 0.573 against 0.9 (unjailed on the same host 0.570) and decay of dax-density 0.883 against 0.9. The dax-gate wasn't run.
- AC2: writes of created files at no-DAX's tps: met (dax 1460 against 1467 median, 0.995). Base first reads at today's DAX's: not measured against a pre-deletion DAX build; bigmap's cold reads are 5.76x no DAX's (96.8 against 16.8 MiB/s), its guest cache 0. Forked pgbench within the gate's threshold: not met (835 against 1459 median).
- Lead: each dax-forked VMM restricted pages to reads for a KVM worker's write some 2^18 times or more (1,009 log lines: the first thousand, then powers of two), each a failed KVM_RUN and retry for the vCPU; likely the forked gap. Density: DAX saves 14% host memory a VM (2,361 against 2,740 MiB), the VMM holding 700 MiB of the daemon's pages and 790 anonymous.

2026-11-06, rewritten to what it means now (the user): 'by default' no longer applies, copy-on-write being the only DAX. The old AC 2's 'base first reads at today's DAX's' is dropped: it compared against DAX before writable DAX was deleted, a build no longer kept nor runnable against main's guest and host; bigmap's cold-read gate (bigmap-read, at least 4x no DAX's; r632 5.76, r650 5.13) is what gates first reads now. Its 'writes of created files at no-DAX's tps' is the dax gate (r650 0.999), its forked pgbench forked-dax at 0.8 (r650 0.836). TASK-164.11 stays open, later work.

Decision moved here from OPEN_QUESTIONS.md (2026-11-06): The user's call: copy-on-write DAX's one host round trip a page's first store (doc-13) is worth its cost. Measured after TASK-164.10, jailed, nested (GCE r650): 0.836 (1,217 against 1,455 tps) and 0.854 on a rerun (1,216 against 1,424), from r649's 0.579; doc-13 had expected 0.9-1.0. `scripts/pgbench-gate.py`'s `forked-dax` threshold is 0.8, those numbers with room for a nested host's noise, and docs/measurements/dax-cow-gate.md and doc-13 say so. Left for later, as TASK-164.11 and its two subtasks: the VMM's request path (~165 µs of every request), and a page made private without replacing its mapping (~135 µs a run, a TLB flush of every vCPU); besides, unmeasured, the private pages' pre-fault off the critical path, `private_max` against the density gate, and the window's fragmentation. *Undoing it:* the threshold back to 0.9, which r650 fails.

2026-11-06, the full gate on main ccbe81d0e (GCE r790 single, r791 density, jailed, nested; docs/measurements/dax-cow-gate.md): not met. AC1: every case passed and every gate but two: forked-dax 0.304 (against 0.8) and decay of dax-forked 0.114 (against 0.9), dax-forked 711, 1023, 439, 116, 81 tps against no-dax-forked's median 1,444. The rest: dax 0.976, static-dax 1.23, decay 0.97-1.39 elsewhere, density 1.07 and 1.24, density-memory 0.871 (2,097.8 MiB a VM added against 2,409.5), density-memory-mib 2,097.8, bigmap-read 5.32, bigmap-guest-cache 0.0, bigmap-written-memory 1.32. Cause, not the forked-perf changes: since TASK-165.8 a fork pages in what it inherited from the bucket, a ranged GET a page-in, one at a time, each with the case's 25 ms of added latency; rerun without it, forked-dax 0.756 and decay 0.92, still a GET a page-in. TASK-164.12 has it, and blocks this. AC2: the dax-gate wasn't run, AC1 failing.

2026-11-06, met (TASK-164.12's fix in: the test daemons keep a part cache, as production's does).
- AC1: the perf gate on GCE r794 (main 76d2c39cf with the fix; single on r792, density on r793, jailed, nested, release guest kernel), every case and gate passing: forked-dax 0.836 (dax-forked 1163, 1221, 1188, 1287, 1211 against no-dax-forked's 1450 median), dax 0.974, static-dax 1.18, decay 1.03-1.24, density 1.03 and 1.02, density-memory 0.847, density-memory-mib 2,097.4, bigmap-read 5.52, bigmap-guest-cache 0.0, bigmap-written-memory 1.32. docs/measurements/dax-cow-gate.md has it.
- AC2: the dax-gate passed at dde984800, noted on it (refs/notes/dax-gate, not pushed), on r799 and r795: fxfs-guest-checks-dax, -delay, -small, fxfs-guest-dax, crash-dax, crash-dax-killed 5 rounds each; pgbench-dax-forked, its probes a map 59 us, a write(2) 428 us; the simulator's mutants 35 killed, 6 none; the real kernel's task-212 and task-79.29 killed, task-79.49 none. Two runs before it failed: (1) pgbench-probes' window MAP request 2,202 us against 560: since TASK-164.10 a request holds ~38 maps, so the probe now takes a map's mean, ceiling 80 us (r794 53; decision in OPEN_QUESTIONS.md); (2) mutant inherited_acls_written_back missed: since TASK-199 (e08d882db), the crash test passed 15 of 15 rounds with it in, so it has a new killer, inherited_acls_on_full_filesystem (killed in round 1, r797; the check passes 3 rounds of fxfs-guest-checks and -dax without it, r798).
- DoD: FXDAX_TEST_PARTS=workspace scripts/test.sh on macOS 2049 passed; Linux by land-p2's CI; upstream-changes.py --check passes; the new check and changed probe are fxdax's own, no upstream code changed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The copy-on-write DAX perf gate and dax-gate pass. Perf gate, GCE r794: forked-dax 0.836, dax 0.974, density-memory 0.847, bigmap-read 5.52, every gate passing. That took TASK-164.12's fix (the test daemons keep a part cache). dax-gate at dde984800: every suite, pgbench-dax-forked's probes, and every mutant killed or none. That took pgbench-probes measuring a map rather than a MAP request (TASK-164.10's batching) and a new killer for inherited_acls_written_back (inherited_acls_on_full_filesystem), which the crash test stopped killing after TASK-199.
<!-- SECTION:FINAL_SUMMARY:END -->
