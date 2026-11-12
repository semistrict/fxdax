---
id: TASK-383
title: >-
  Simulation: stale_creates_landing_past_deletions seed 453 (SIM_SOAK=5) loses
  branch 2's acknowledged commits 27-29
status: To Do
assignee: []
created_date: '2026-11-12 12:39'
labels:
  - bug
  - simulation
  - collector
dependencies: []
priority: high
type: bug
ordinal: 762000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found 2026-11-12 by TASK-380's soak, once seeds replay (16b9f306a): extent_store sim profile recreated (stale_creates_landing_past_deletions), seed 453, which only SIM_SOAK>=3 reaches (CI runs 0..200). Deterministic: SIM_SEED=453 SIM_PROFILE=recreated cargo nextest run --cargo-profile sim -p extent_store -E 'test(=sim::one)' --run-ignored only fails every run with 'branch 2 lost commit 27/28/29, acknowledged'; SIM_TRACE=1 prints each bucket request. Not TASK-380's rule: it fails with SIM_OFF=deletions_reread too. Candidates: TASK-371's acknowledgement without a lease read (fences_kept, takeovers_kept), as the profile holds stale creates past takeovers and compacts.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The seed's loss understood and its rule or code fixed, with a direct test that loses the commits without the fix
- [ ] #2 The profile passes at SIM_SOAK=20
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
