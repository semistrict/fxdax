---
id: TASK-160.27
title: CI runs what the repo already tests
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-03 13:09'
updated_date: '2026-11-03 13:31'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 598000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
CI runs cargo nextest --workspace alone, not scripts/test.sh (the clippy check, 6 Shuttle suites, 25 Python test files under scripts/tests). guest/tests' host-feature tests (vm.rs, script_vm.rs) run nowhere. guest/sim's zloop seeds interleave only, not failures or smoke; its test lists are kept by hand in lib.sh, check.sh and mutants.sh. A mutant patch that stops applying drops silently. Explains 0a3843c8a (test_each_edit_finds_its_code_once never ran), bf82dcb37, TASK-120, 127. Proposal: CI calls scripts/test.sh; a guest/tests job per feature set; lib.sh lists tests from tests/*.c; zloop seeds failures too; a --check mode for every mutant runner.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 CI runs scripts/test.sh and guest/tests' host tests
- [ ] #2 zloop seeds failures; the test list derived
- [ ] #3 A broken mutant edit fails CI
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Check the review's claims: CI runs nextest alone (true); guest/tests host tests run nowhere (true: test-e2e runs --features guest only, CI checks host without --tests); zloop seeds interleave only (true); lib.sh list by hand, missing smoke (true); a patch that stops applying drops silently (true for mutants.sh, fxfs-posix/mutants.sh; 7 patches need fuzz now).
2. guest/sim: lib.sh derives one-run tests from tests/*.c (seeded: interleave, failures); zloop seeds both; check.sh uses lib.sh; mutants.sh --check (apply, reverse, compare, no fuzz), BROKEN rows fail it; known-failures counted apart.
3. fxfs-posix/mutants.sh --check; guest-mutants-gce.py --check; refresh drifted patches.
4. scripts/test.sh parts (FXDAX_TEST_PARTS), guest part on Linux; CI a matrix of its parts; guest-sim runs mutants --check.
5. Prove on a branch: CI green, a broken mutant edit red.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Claims checked (2026-11-03): CI ran cargo nextest --workspace alone, true (ci.yml); guest/tests host-feature tests ran nowhere, true: test-e2e.sh runs --features guest only, and CI's cargo check --features host compiled no test code; zloop seeded interleave only, true, and lib.sh's list missed smoke, which zloop and mutants.sh never ran; check.sh ran two of the nine one-run tests. A patch that stopped applying: mutants.sh printed to stderr and exited 0; fxfs-posix/mutants.sh went on, and never checked its reverse. scripts/tests/test_guest_mutants.py did check the Python runner's edits, but only scripts/test.sh ran it, which CI didn't. Strictly (no fuzz) 7 patches no longer applied: guest/sim task-147-growing, task-150.20-no-seal-zero, task-153.6-through-under-a-fault, task-79.28-before-147, write-past-end-grows (malformed hunk), and fxfs-posix clone-counts-holes; all applied with fuzz, to the code they meant: regenerated.
<!-- SECTION:NOTES:END -->
