---
id: TASK-160.27
title: CI runs what the repo already tests
status: Done
assignee:
  - '@claude'
created_date: '2026-11-03 13:09'
updated_date: '2026-11-06 06:56'
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
- [x] #1 CI runs scripts/test.sh and guest/tests' host tests
- [x] #2 zloop seeds failures; the test list derived
- [x] #3 A broken mutant edit fails CI
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
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

Landed on main 259476d8b (rebased onto TASK-150.21's fix, so guest/sim/known-failures lists nothing now). CI is a matrix of scripts/test.sh's parts (FXDAX_TEST_PARTS): workspace ~30 min, shuttle ~14, guest (as root: script_run's tests mount loop devices, mkfs.ext4/xfs) ~3, clippy+scripts ~2; guest-sim ~27 min (failures' seeds are 3.5x interleave's). What first CI found: test_cocci_guest needs spatch on the runner (coccinelle added to apt), and script_run::tests::a_filesystem_with_no_room... needs root to mount: neither a bug, both environment, so CI runs the guest part as root as GCE does. Nothing else that had gone unrun failed: vm.rs, script_vm.rs, all 56 host and 53 guest tests pass; zloop's new failures seeds 1-3,000 clean; smoke clean. Proofs: branch run 41650394074 green (all 5 jobs); run 41657042767 with task-99-region-open.patch's context changed by one line (which fuzz 2 would have applied) and map_to_unlocked's edit changed: guest-sim red ('task-99-region-open: mutants/task-99-region-open.patch doesn't apply', 'mutants: 36 checked, 1 broken'), clippy-scripts red (test_each_edit_finds_its_code_once: 'map_to_unlocked: ... found 0 times'). main's CI 41657583988 green.

DoD 1 not checked: Linux ran on GitHub's runners (CI 41657583988, every part green), not GCE; on macOS only fxfs-posix's tests and the scripts' part ran. upstream-changes.py --check: exit 0. Structure: the class (tests and patches nobody runs) came of two hand-kept lists and a CI that wasn't the local suite; both gone: lists derived, CI is scripts/test.sh.

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**CI runs scripts/test.sh, a part a job; open bugs' failures are listed, not failed on (TASK-160.27, .28, 2026-11-03)**

CI ran `cargo nextest --workspace` alone. Decided: CI runs `scripts/test.sh`, which now has parts (`FXDAX_TEST_PARTS`: clippy, workspace, shuttle, scripts, and guest on Linux), as four jobs at once: the workspace, the Shuttle suites, the guest tests' own (as root, as on GCE: some mount loop devices), and the rest. The guest part runs guest/tests' own tests, both feature sets (vm.rs, script_vm.rs ran nowhere); scripts runs the mutant runners' `--check`. Every hand mutant's patch applies with no fuzz now, or fails `--check` (7 needed fuzz, one was malformed: refreshed). Open bugs' failures go in a file next to what meets them, `guest/sim/known-failures` (a regex of zloop's failure line) and `crates/fxfs-posix/known-panics` (a panic's place, which `corrupt_tests` counts apart); both are read on every run, CI's too, not only by the hunt, so that one bug fails nothing until its fix takes its line out. A nightly `hunt.yml`, CPU only: zloop over 20,000 new seeds (4 shards), the model and corruption tests at 50x (4 shards, seeds from the run's number); each night some 6 runner-hours, free on a public repo.
**Instead:** one known-failures file for both (their formats differ); known signatures only in the hunt (CI's 3,000 seeds then fail as often as an open bug shows: TASK-150.21's WARN, about one seed in 6,000, until patch 0012 fixed it); a hunt on GCE (needs the nightly's credentials, unset).
**Undoing it:** CI back to its `cargo nextest` step; drop `hunt.yml`; empty the two files. guest/sim's lists none now.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
CI now runs scripts/test.sh, a part a job (workspace, shuttle, clippy+scripts, guest as root), so CI and local run the same; guest/tests' host and guest tests (vm.rs, script_vm.rs) run in it. guest/sim derives its one-run tests from tests/*.c (smoke and every new file run), zloop seeds failures as well as interleave, check.sh uses the same lists. Every mutant runner has --check (apply and reverse, no fuzz), run in CI; 7 drifted patches regenerated. Verified: CI 41657583988 on main green; 41657042767 with a mutant patch's context and a mutant's edit broken went red in guest-sim and clippy-scripts.
<!-- SECTION:FINAL_SUMMARY:END -->
