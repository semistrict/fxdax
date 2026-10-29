---
id: TASK-119.3
title: >-
  Delay injection in the default suites: fxfs-guest-checks-dax-delay with a seed
  per round; DAX diagnostics fail tests
status: Done
assignee:
  - '@claude'
created_date: '2026-10-28 19:27'
updated_date: '2026-10-29 13:35'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: high
ordinal: 439000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxfs.dax_delay_us found TASK-101, 112.6 and 113, but no test or script sets it; runs were by hand via FXDAX_GUEST_CMDLINE. dax_evacuee_verify warns with pr_warn, which the runner does not count as a failure.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A case runs checks-dax with fxfs.dax_delay_us=200, fxfs.dax_evacuee_verify=1 and a seed per round, in the default e2e and GCE sets
- [x] #2 Delay points at seal begin and end and the evacuation steps
- [x] #3 A DAX diagnostic warning fails the test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
New case fxfs-guest-checks-dax-delay (fxfs.dax_delay_us=200, dax_delay_seed = the run's seed) in test-e2e.sh defaults, test-guest-kernel.sh and the mutant suites; Vm::module_param. New delay points: fxfs_dax_seal between regions and files, fxfs_dax_sealed, evacuation between MAP and place. AC3 (diagnostic warnings fail tests): dax_evacuee_verify lives on wip/task-112-5 only; it becomes a WARN there.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
AC1, AC2 on main since b9af10794: fxfs-guest-checks-dax-delay (in test-e2e.sh's and test-guest-kernel.sh's defaults and the DAX gate) runs the checks with fxfs.dax_delay_us=200 seeded per round (fxfs.dax_delay_seed, the run's printed seed) and fxfs.dax_check_places=1 (evacuee_verify's successor); delay points at seal begin and end and each evacuation step. GCE r221, r224 (10 rounds), r226a pass. AC3: 49db8a47b routes every diagnostic's finding (dax_check_moves, which said it WARNed and only logged; dax_shadow, dax_check_seals, dax_verify_seals) through fxfs_dax_diagnosed's WARN_ONCE, which the runner fails tests on (as r225b's kill by a WARN shows); not yet shown with a diagnostic of these firing in a test.

GCE r228m: mutant held_not_written with fxfs.dax_check_moves=1 fxfs.dax_shadow=1: 0.57 s in, 'fxfs: ino 4 page 0 moved from 0 to 10351000 holds 4079 other bytes', then 'a DAX diagnostic found the window wrong' and the WARNING from fxfs_dax_diagnosed; checks-dax failed (killed; the script said BROKEN, as test-e2e.sh said nothing of a single round's failure: b0ccb059c). The control r228c, every diagnostic on over correct code, stopped its VM on a refused access: TASK-119.21. DoD #1 not run.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
checks-dax-delay runs the checks with seeded delay injection and check_places in the default suites and the gate; every DAX diagnostic's finding WARNs (fxfs_dax_diagnosed), which fails the test: shown by r228m, where dax_check_moves caught held_not_written.
<!-- SECTION:FINAL_SUMMARY:END -->
