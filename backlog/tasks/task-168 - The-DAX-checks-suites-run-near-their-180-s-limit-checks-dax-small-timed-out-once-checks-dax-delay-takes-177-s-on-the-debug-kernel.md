---
id: TASK-168
title: >-
  The DAX checks suites run near their 180 s limit: checks-dax-small timed out
  once, checks-dax-delay takes 177 s on the debug kernel
status: Done
assignee:
  - '@claude'
created_date: '2026-11-04 09:18'
updated_date: '2026-11-06 06:56'
labels: []
dependencies: []
priority: medium
type: task
ordinal: 621000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r533 (batch 2026-11-04 at 9e92e6925): fxfs-guest-checks-dax-small timed out at 180 s, in enospc, its guest reading and reclaiming window ranges (fxfs_dax_read_iter, reclaim_range, device_unmap) after window_held_killed passed at 120 s, where main's r532 passed it at 102 s. Four rounds since, two with TASK-160.22 (r535) and two without (r536), all pass: not reproduced, a suite near its limit. The debug kernel's checks-dax-delay took 177 of 180 s (TASK-44.8). A timeout that a slower host or one more check crosses fails runs for nothing, and hides a real slowdown among them. Options: split the checks into suites of their own (TASK-160.30, each check in a process of its own), or time each check rather than the suite, with a budget per check; record each check's time in the results so a slowdown shows.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 No checks suite runs within 20% of its limit on GCE n2-standard-4, release and debug kernels
- [x] #2 Each check's time is in the results
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. One declarative table of the checks (name, suites, budget, body) that host and guest share (X-macro), replacing the guest's hand-ordered list.
2. The guest times each check (PASS/FAIL: name (took of budget) in the log) and a watchdog ends the run, naming the check and dumping the guest's stacks, when one outruns its budget.
3. The host derives the suite's limit from its checks' budgets, scaled by the guest kernel's kind (FXDAX_GUEST_KERNEL_BUILD, exported by test-linux-host.sh), object storage and FXDAX_CHECKS_SCALE; unknown FXDAX_CHECKS names fail.
4. The runner prints each test's time of its limit; fxfs-guest-checks-overrun tests the watchdog.
5. GCE: measure per-check times on release and debug kernels, set budgets from them, re-run both to verify no suite within 20% of its limit.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05, on task-168 (d7cd44066, a3e3a99c0, rebased on 09b36991a).

Design: one table of the checks, which host and guest share (test_fxfs_guest_checks.rs, an X-macro `checks!` making the host's CHECKS and the guest's bodies of them): name, the suites that run it (Suites), budget, body; it replaces the guest's hand-ordered list, its order pinned by suites_run_their_checks_in_order. The guest times each check, `PASS: name (took of budget)` in the log, and a watchdog thread, as a check runs on past its budget, dumps the guest's stacks (kernel_log::stacks, sysrq l, w, t, now shared with guest_init's watchdog), says `FAILED: <check>, still running after its budget of …` straight to the console (guest_init::complain_out, say_out) and resets the VM: a hang or a slowdown fails by name, at once. The suite's limit is its checks' budgets plus 30 s (SETUP), a backstop; timed_out names the check the guest was in. Budgets: twice the most a check took on the release kernel (r560 page cache, r563 DAX), 3 s at least; scaled by common::guest_kernel_slowness (FXDAX_GUEST_KERNEL_BUILD, exported by test-linux-host.sh: debug 1.5, measured 1.37 at most for checks of a second or more; rust-checks and coverage 1.5 and KASAN 4 guessed), by 2 for FXDAX_EXTENTD_REMOTE, and by FXDAX_CHECKS_SCALE. FXDAX_CHECKS naming no check fails (TASK-160.30's AC2). fxfs-guest-checks-overrun runs enospc on a hundredth of its budget and passes on the guest's FAILED line: the watchdog's path exercised, in test-e2e.sh's and test-guest-kernel.sh's defaults. The runner says each test's time of its limit ("OK (72 s of 373 s)"). Decision recorded in OPEN_QUESTIONS.md.

GCE (n2-standard-4): r560 release us-central1-a and r561 debug us-west1-b: every DAX suite's VM killed by seccomp at its first store (pread64 in extentd client, missing from vendor/firecracker at 9f81f6018, fixed in 09b36991a); page-cache suite 62.5 s release, 65.5 s debug; overrun passed. r562 debug and r563 release, FXDAX_FIRECRACKER_SECCOMP=log, DAX suites: 84-89 s debug, 71-73 s release, one call refused (extentd client, 17, 762 times). r564 release, real filter, on 09b36991a with the budgets: checks 61 of 291 s, checks-dax 72 of 373 s, -delay 74 of 373 s, -small 73 of 368 s, overrun 4 of 30 s; every check passed; nearest a budget: enospc 17.2 of 33 s, log_writes 10.2 of 21 s (49-52%). r565 debug, real filter: checks 76 of 421 s, -dax 85 of 544 s, -delay 85 of 544 s, -small 85 of 537 s, overrun 11 of 30 s; every check passed; nearest a budget freeze 2.52 of 4.5 s (56%, an outlier: 0.6 s elsewhere), enospc 20.6 of 49.5 s. fxfs-guest-checks failed on its console: KCSAN's fxfs_vfs_read_folios / lru_add, the reviewed folio-flags race, lru_add inlined above folio_batch_move_lru: added to KCSAN_REVIEWED_PEERS (a3e3a99c0), its test red without it. The DAX suites now take about 72 s release, not the 140-177 s of the description: copy-on-write DAX, after writable DAX's deletion. The largest checks: enospc 15-17 s release, 20-22 s debug; torture 13-19 s; log_writes and writes_across_seals 10 s (fixed deadlines); faults_writes_compaction 8 s; random_ops 3.5-4 s; the rest under 2.

Tests: guest/tests' unit tests in Lima: host half 60/60, guest half 57/57 as root (3 loop-device tests need root); the table's 4 tests on both halves; vm:: 11/11. macOS: FXDAX_TEST_PARTS=scripts scripts/test.sh, 151 tests and the mutant checks pass. Not run: KASAN, coverage and rust-checks kernels (their factors guessed); cargo nextest --workspace (DoD 1), unchanged code.

Structure: the class of bug, a suite outrunning one limit that named no check, is gone: a limit per check, the one table owning order, suites and budgets, which host and guest can't disagree on. Left of TASK-160.30: checks still share PID 1, a crash takes the rest.

GCE r584 on main c919610e7: Linux workspace 2249/2249 and every other part pass; macOS 2068/2068 + Shuttle on the landed stack; CI 41994971122; scripts/upstream-changes.py --check passes. r564 (release) and r565 (debug), real filter: every checks suite at 16-21% of its limit; r583 on the landed stack: checks 61/291 s, -dax 73/373, -delay 73/373, -small 74/368, overrun 4/30. r565's one failure was KCSAN's lru_add name, reviewed in a4c7cdf06.

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**A budget per guest check, its suite's limit their sum (TASK-168, 2026-11-05)**

The checks suites ran under one fixed 180 s limit each, near it at times. Decided: each check has a budget in one table the host and guest share (`test_fxfs_guest_checks.rs`'s `checks!`): twice the most it took on the release kernel on GCE n2-standard-4 (r560's page cache, r563's DAX), at least 3 s. A check still running past its budget makes the guest dump its stacks, say `FAILED: <check>, still running after its budget of …` and reset the VM; the suite's limit is its checks' budgets plus 30 s, a backstop for a guest too stuck to. Budgets scale by the kernel's kind (`common::guest_kernel_slowness`: debug 1.5, measured at 1.37 at most; rust-checks and coverage 1.5, and KASAN 4, guessed), by 2 for object storage, and by `FXDAX_CHECKS_SCALE`. At this, the suites run at 16-21% of their limits (r564 release, r565 debug).
**Instead:** suites split into more VMs, each under the old 180 s (a split that drifts as checks are added, naming no slow check); a check failing over its budget only once it ends (a hang still waits for the suite's limit).
**Undoing it:** the budgets are numbers in the table; a check failing on its budget too often has its number raised, or `FXDAX_CHECKS_SCALE` set for a slower host.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
One table of guest checks, each with a budget (twice its slowest release time, 3 s at least, scaled for debug 1.5x measured, KASAN/coverage guessed, object storage, FXDAX_CHECKS_SCALE); the guest times each check and a watchdog ends the run naming one past its budget; a suite's limit is its budgets' sum plus 30 s; FXDAX_CHECKS naming nothing fails; fxfs-guest-checks-overrun proves the watchdog. Every suite at 16-21% of its limit on release and debug (r564, r565, r583); each check's time is in the log. GCE r584 on main c919610e7: Linux workspace 2249/2249 and every other part pass; macOS 2068/2068 + Shuttle on the landed stack; CI 41994971122; scripts/upstream-changes.py --check passes.
<!-- SECTION:FINAL_SUMMARY:END -->
