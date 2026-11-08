---
id: TASK-248
title: >-
  CI: guest/sim's seeds and sweep a smoke run unless what they test changed;
  whole nightly
status: Done
assignee:
  - '@claude'
created_date: '2026-11-08 08:54'
updated_date: '2026-11-08 11:22'
labels:
  - ci
dependencies: []
priority: medium
ordinal: 681000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The user's decision, 2026-11-08 (OPEN_QUESTIONS.md, d1a7a77f3): guest-sim (3,000 zloop seeds, ~30 min) and guest-sim-sweep (~31 min) set CI's wall time (~35 min against ~16); since 2026-10-31 (430 runs) guest-sim failed 8 times, each beside workspace or shuttle failures of the 11-02/11-04 rewrite, and the sweep never.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 CI runs them whole when guest/linux/fs/fxfs/, guest/linux/patches/, guest/sim/ or scripts/build-guest-kernel.sh changed since the base, a smoke run otherwise (~200 seeds, a rotating slice of the sweep, mutants.sh --check), job names kept
- [x] #2 The nightly workflow runs the 3,000 seeds and the whole sweep
- [x] #3 The full-or-smoke decision is a script with a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
scripts/guest-sim-scope.py from the changed paths (the base as firecracker-forward finds it; a dispatch input overrides); sweep.sh -s K/N, every Nth call of each run, K the run number, N 12; ci.yml's jobs; nightly.yml's whole runs; test_guest_sim_scope, test_ci.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-08: land-h20 ec77d2c11 (on land-all13), CI 42464910881 dispatched with guest-sim=smoke, as the branch itself changes guest/sim (auto would run it whole). Local (Docker, 15 cores): sweep.sh whole 195 s, -s K/12 21 s (1,445-1,479 of 17,618 calls).

Verified 2026-11-08: on main, GitHub CI 42469441083 green at 08d444ef7 (macOS and Linux workspace, shuttle, sim, guest-sim, firecracker), scripts/upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
CI runs guest/sim's jobs whole only when guest/linux/fs/fxfs/, guest/linux/patches/, guest/sim/ or scripts/build-guest-kernel.sh changed since the base, a smoke run otherwise; the nightly runs the 3,000 seeds and the whole sweep; scripts/guest-sim-scope.py decides, tested by test_guest_sim_scope and test_ci (096fa2233).
<!-- SECTION:FINAL_SUMMARY:END -->
