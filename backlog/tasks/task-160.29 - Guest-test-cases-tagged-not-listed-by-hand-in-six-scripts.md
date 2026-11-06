---
id: TASK-160.29
title: 'Guest test cases tagged, not listed by hand in six scripts'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-03 13:09'
updated_date: '2026-11-06 02:22'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 600000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
What runs is listed by hand in test-e2e.sh, test-guest-kernel.sh, dax-gate.sh, guest-mutants-gce.py, soak-workloads-gce.sh and perf-gate-gce.sh; xfstests, the script cases, crash-dax-daemon-killed and pgbench variants are named by none; nightly runs no follow, snapshot, volumes, faults-dax, pjdfstest or xfstests case. Explains 3981ad18f, 6d6f1846c, 69227bac0. Proposal: tags on TestCase and runner list --tag; a unit test fails an untagged case.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Cases tagged; scripts take tags
- [x] #2 A unit test fails an untagged case
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Tag enum on TestCase (lib.rs, a macro: variant, name, doc as help); every case tagged; select() of names and tags in registration order.
2. runner list [--tag TAG]... [NAME]...: the selection, a name a line; unknown tags refused by clap, unknown names and empty selections fail.
3. test-e2e.sh builds the runner first and runs what its arguments select (default --tag e2e); scripts pass --tag through FXDAX_E2E_TEST (test-gce.sh's regex already takes it): test-guest-kernel (kernel), dax-gate (dax-gate, dax-gate-mutants), guest-mutants-gce (mutants), soak-workloads-gce (soak-GROUP), perf-gate-gce (perf-single, perf-density), bench.sh (bench), nightly (nightly).
4. Tests: lib.rs every_case_is_in_a_suite (red with an untagged case), every_tag_selects_cases, selection; scripts/tests/test_guest_tags.py: every --tag a script names is a tag, every tag but manual is run.
5. Nightly: add kernel's cases plus cheap others (crash-dax-daemon-killed, follow-remount(-dax), script cases).
6. Verify: runner list per tag against the old lists; scripts/test.sh clippy scripts guest in Docker; GCE run.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Design: `Tag` (guest/tests/test_cases/src/lib.rs, declared by a `tags!` macro: variant, name, doc as clap help) on every `TestCase::new(name, tags, test)`; `select` takes names and tags, in registration order, each case once, an unknown name an error. `runner list [--tag TAG]... [NAME]...` prints them; clap refuses an unknown tag, listing the suites; an empty selection fails. test-e2e.sh builds the runner first and runs what its arguments select (default `--tag e2e`); the scripts pass `--tag SUITE` through FXDAX_E2E_TEST, which test-gce.sh's regex already took. Suites: e2e, kernel, nightly, dax-gate, dax-gate-mutants, mutants, soak-crash/checks/sanitized/replicas, perf-single, perf-density, bench, manual. bench.sh's FXDAX_BENCH_CASES default was a seventh hand list: `--tag bench`. FXDAX_E2E_TEST=all is gone (it would now fail on runner-deadlock and other skips).

Blind spot: each script kept its own list, which no test compared with the cases; a new case was in none (xfstests, the script cases, crash-dax-daemon-killed, pgbench variants), and the nightly ran test-e2e.sh's default, 15 cases. Tests: lib.rs every_case_is_in_a_suite (red with an untagged case pushed: it asserts that too), every_tag_selects_cases, a_selection_is_of_cases_named_and_tagged; scripts/tests/test_guest_tags.py: every `--tag` a script or workflow names is a declared tag, every tag but manual is run by one; test_soak_workloads: each group soaks soak-GROUP; test_dax_gate and test_perf_gate_gce on the tags.

Selections, `runner list --tag` against each old list (Docker, arm64): test-e2e default 15=15, test-guest-kernel 39=39, dax-gate 6=6, dax-gate mutants 3=3, guest-mutants-gce 4=4, soak crash 3=3, checks 4=4, sanitized 3=3, replicas 6=6, perf single 7=7, density 2=2, bench 4=4: same sets; order is now registration order.

Nightly missed: every follow, snapshot, suspend, migrate, volumes, import, resize, live-fork, daemon-lost, format, pjdfstest, script and xfstests case, and faults-dax. Now `--tag nightly`, 50 cases: e2e's and kernel's plus crash-dax-daemon-killed, follow-remount(-dax) and the four script cases (180 s limits each). Not added: xfstests (4 h each), faults-dax (needs the debug kernel; soak-sanitized has it), the measurements. The nightly still has no GCP credentials (OPEN_QUESTIONS.md), so this is unrun there.

Structure: fine as it is now; one owner of where a case runs, the case's registration.

Verified: FXDAX_TEST_PARTS="clippy scripts guest" scripts/test.sh in Docker (rust:1, root): clippy clean, 159 script tests, test_cases host 63/63, guest 60/60, runner 4/4.

GCE r605 (us-central1-a, release kernel, FXDAX_E2E_TEST by name, as the brief asked): test-e2e.sh asked the runner, which ran the three cases in registration order (checks-dax-small, checks-overrun, dax), all OK (74 s of 368 s, 4 s of 30 s, 2 s of 15 s); guest tests exit 0. Tag selections themselves checked against the old lists in Docker (above); no GCE run of a --tag suite yet.

Verified on main, 2026-11-05 (code of dde8d58d3; 752515020 adds backlog only): GCE r691, the Linux workspace, 2302 of 2303, the one failure TASK-187's a_store_after_a_workers_write_waits_for_one_lift (TASK-190; its fix 788e3b6b4 is on land-p, not main); GCE r690, the e2e suite (--tag e2e, 15 cases) jailed with the production seccomp filter, 15 of 15; CI 42137709782 on dde8d58d3 green (workspace 2303 of 2303, sim, shuttle, clippy scripts, guest, guest-sim); macOS FXDAX_TEST_PARTS=workspace scripts/test.sh at 752515020, 2118 of 2118; scripts/upstream-changes.py --check at 752515020 passes. AC #1: GCE r690 ran the e2e suite by tag (FXDAX_E2E_TEST='--tag e2e'), the runner selecting its 15 cases, 15 of 15; scripts/tests/test_guest_tags.py passes in CI's scripts part (163 tests). AC #2: test_cases' every_case_is_in_a_suite, every_tag_selects_cases and a_selection_is_of_cases_named_and_tagged pass in CI 42137709782's guest job, host and guest halves.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Each guest case is tagged with its suites; runner list --tag selects them and every script runs by tag; a unit test fails an untagged case. On main: aad1ba65b. Verified: GCE r690 by --tag e2e, 15 of 15; the tag tests in CI 42137709782's guest job; GCE r691 (workspace 2302 of 2303, the failure TASK-190's), CI 42137709782, macOS workspace 2118 of 2118 at 752515020.
<!-- SECTION:FINAL_SUMMARY:END -->
