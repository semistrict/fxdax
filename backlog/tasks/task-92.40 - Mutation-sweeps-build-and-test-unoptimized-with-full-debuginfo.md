---
id: TASK-92.40
title: 'Mutation sweeps build and test unoptimized, with full debuginfo'
status: To Do
assignee: []
created_date: '2026-10-22 21:06'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 315000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
There is no [profile.*] in Cargo.toml, none in .cargo/mutants.toml, and no --profile in mutants.sh:63-64, 71-73, so every mutant runs at opt-level 0 with full DWARF. Meanwhile the CPU-bound simulation runs 200 to 20,000 seeds per test (sim/mod.rs:55-75), and timeouts scale from that slow baseline. The extent_remote sweep did 569 of 611 mutants in 7 h at about 4 min each, with 17 timeouts costing about 1.9 h.

Where: scripts/mutants.sh:63-64, scripts/mutants.sh:71-73, .cargo/mutants.toml:1-10, crates/extent_remote/src/sim/mod.rs:55-75.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present. Cargo.toml has no [profile.*], and .cargo/mutants.toml sets only exclude_globs/exclude_re with no profile. scripts/mutants.sh's cargo mutants calls pass no --profile (mutants.sh:63-64, 71-73). Not tracked. TASK-44.1's notes record the sweep being stopped as too slow (~4 min per extent_remote mutant). Related: TASK-44.1, TASK-44.5, TASK-79.63.
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
