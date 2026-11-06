---
id: TASK-92.40
title: 'Mutation sweeps build and test unoptimized, with full debuginfo'
status: To Do
assignee: []
created_date: '2026-10-22 21:06'
updated_date: '2026-11-06 06:51'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: low
type: enhancement
ordinal: 315000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
At dde8d58d3. Cargo.toml has one profile now, sim (Cargo.toml:146-153: release with debug assertions and overflow checks, TASK-188, ebe4f4682), which scripts/test.sh's sim part builds extent_remote's simulation with (scripts/test.sh:45-56). The mutation sweeps don't use it: .cargo/mutants.toml sets only exclude_globs and exclude_re, and scripts/mutants.sh's cargo mutants call (mutants.sh:78) passes no --profile or --cargo-profile, so every mutant builds at opt-level 0 with full DWARF. Meanwhile the CPU-bound simulation runs 200 seeds per profile test and up to 20,000 per rule test (crates/extent_remote/src/sim/mod.rs:55-74), whose seed searches took 20 to 27 minutes each unoptimized (TASK-188), and timeouts scale from that slow baseline. The extent_remote sweep did 569 of 611 mutants in 7 h at about 4 min each, with 17 timeouts costing about 1.9 h.

Where: scripts/mutants.sh:78, .cargo/mutants.toml:1-11, Cargo.toml:146-153, crates/extent_remote/src/sim/mod.rs:55-74.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. TASK-44.1's notes record the sweep being stopped as too slow (~4 min per extent_remote mutant). Related: TASK-44.1, TASK-44.5, TASK-79.63, TASK-188.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [ ] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05 triage: description brought to dde8d58d3: Cargo.toml has [profile.sim] since ebe4f4682 (TASK-188), which the sweeps don't use; mutants.sh's line numbers moved.

2026-11-06 tidy: priority to low: whole mutation sweeps were stopped by the user on 2026-10-19 (dead-ends/full-cargo-mutants-sweep.md); TASK-92.40 is what would make resuming TASK-44.1 cheaper.
<!-- SECTION:NOTES:END -->
