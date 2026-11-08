---
id: TASK-244
title: 'scripts/license-headers.py fails on main, and CI never runs it'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-08 06:25'
updated_date: '2026-11-08 06:25'
labels:
  - scripts
dependencies: []
priority: medium
ordinal: 678000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in the TASK-149/44 sweep (2026-11-07): the script reported 160 problems on main: the hand mutants' patches (crates/*/mutants, guest/mutants) and the fxfs-sim-diff shim's files; and without the Fuchsia checkout, as in CI, it exited at the 44 files citing Fuchsia. Nothing ran it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 license-headers.py passes on main, with and without the Fuchsia checkout
- [x] #2 CI runs it, with a test of the script
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Exempt */mutants/*.patch (their leading comments are the mutant's fields); fxfs-sim-diff's shim under guest/sim's dual license; without the checkout, files citing Fuchsia need its copyright and one notice; the scripts part runs it; test_license_headers.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-08: land-h13 on main as 25a858900 (land-all4, 722ce0723), CI 42449438764 green, its clippy scripts part running license-headers.py and test_license_headers (4 of 5 red on the old script).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
license-headers.py passes on main and in CI's scripts part: mutant patches exempt with the reason in the script, crates/fxfs-sim-diff/shim (fxdax's own) held to guest/sim's GPL-2.0 OR BSD-3-Clause, files citing Fuchsia checked for its copyright and one notice where the checkout is absent. Verified by scripts/tests/test_license_headers.py (red on the old script) and CI 42449438764.
<!-- SECTION:FINAL_SUMMARY:END -->
