---
id: TASK-160.30
title: Guest checks each in a process of its own
status: Done
assignee:
  - '@claude'
created_date: '2026-11-03 13:09'
updated_date: '2026-11-06 06:56'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 601000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
About 90 checks run in PID 1 under catch_unwind: a crash takes the VM, so checks are ordered by hand; state leaks between them (unmap_what_it_left, Param and Pauses guards); an unknown name in FXDAX_CHECKS runs nothing and passes. Explains TASK-119.18, 3a9e74bcd, 2e1a2b225, 9c0f768c9. Proposal: each check a declaration with when it applies and an optional host-side verify; the agent re-runs itself per check with a deadline that dumps stacks; parameters reset in one place; unknown names fail; the 4,900-line file split by area.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A check that SIGBUSes or hangs fails by name, the rest pass
- [x] #2 FXDAX_CHECKS=nope fails
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Split test_fxfs_guest_checks.rs by area, a refactor commit of its own: the table (mod.rs), host.rs, guest/mod.rs (shared helpers), guest/<area>.rs.
2. The agent re-runs itself per check (FXDAX_CHECK names it), in a process group of its own, to its budget (TASK-168): past it, stacks dumped, the group killed.
3. One place puts back what a check left (Guest::put_back): fxfs's writable module parameters as booted, the filesystem thawed, mounted; before the agent waits for the group, so a task stuck on what it left goes on; the VM reset if the group outlives 10 s.
4. Table entries gain an optional host verify (VERIFIES, as BODIES on the guest), run for the checks that ran.
5. fxfs-guest-checks-overrun: two checks made to fail (crashes: SIGBUS with a fault injection left set; hangs: a write into a frozen filesystem) before the page cache's checks; the host checks each failed alone, as made to, its state put back, the rest passed.
6. Unknown FXDAX_CHECKS names fail: done by TASK-168, kept.
7. Verify: guest tests' unit tests in Docker (Linux), run_to's red/green; GCE r610 release and r611 debug, every checks suite.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05, on task-160-30 (from origin/land-157): e7313d14b (the split), f9ed22780 (Cargo.lock), 38691000f (each check in a process of its own), 07cb7b9d6 (the crash test without a core).

Design: the agent (PID 1) runs itself again per check, FXDAX_CHECK naming it (guest/run.rs), in a process group of its own, to the check's budget (TASK-168). Past it: the guest's stacks (kernel_log::stacks) on the console, the group killed. Then Guest::put_back, the one place that resets state: fxfs's readable and writable module parameters back to the values the guest booted with, the filesystem thawed (FITHAW), mounted again; each logged as '<check>: left ...'. Only then does it wait for the group, its orphans the agent's (PR_SET_CHILD_SUBREAPER), so a task waiting in the kernel on what the check left (a frozen filesystem, a pause point) goes on and ends. A group still there 10 s later resets the VM, as a hang did before. Log: 'FAIL: <check> (took of budget): killed by signal 7 (Bus error) | exited with 101 | still running after its budget: killed'. Param stays a guard for a part of a check; Pauses' guard became a Param on dax_pause; unmap_what_it_left and the in-process Watchdog are gone. The table entries are declarations: suites (when it applies), budget, body and an optional host verify (VERIFIES on the host, BODIES on the guest, both from checks!). The verifies run for the checks that ran, FXDAX_CHECKS's too, where before the host checked only whole suites. Unknown FXDAX_CHECKS names already failed (TASK-168): kept, unknown_names_fail. fxfs-guest-checks-overrun now runs Suites::Failing, crashes (SIGBUS past a mapped file's end, dax_fail_maps left at 1000) and hangs (a write into a frozen filesystem), then the page cache suite. The host requires exactly 'FAILED: crashes, hangs', each one's FAIL line and its put-back line, and the host verifies of the rest. Cost: the overrun case takes about 60 s instead of 4 s (OPEN_QUESTIONS decision).

Tests: guest tests' unit tests in Docker (Linux, aarch64, root): host half 62/62, guest half 63/63. run_to: passes/fails by status, crashes by signal, overruns killed, leftovers killed. Red: without the group's kill, leftovers_are_killed and overruns_are_killed fail (Stuck after 10 s). upstream-changes.py --check passes. Not run: cargo nextest --workspace (no workspace code changed).

GCE (n2-standard-4, jailed Firecracker): r610 release, us-east4-a, 38691000f: all 5 suites OK: checks 58 of 291 s, -dax 66 of 373 s, -delay 67 of 373 s, -small 68 of 368 s, overrun 61 of 297 s. In the overrun suite: 'FAIL: crashes (3.67ms of 3.00s): killed by signal 7 (Bus error)', 'crashes: left fxfs.dax_fail_maps at 1000, set back to 0', 'FAIL: hangs (6.27s of 3.00s): still running after its budget: killed', 'hangs: left the filesystem frozen, thawed', and the 51 page cache checks PASS after them. The host reported a dash core: run_to's own unit test (kill -BUS $$), which test-e2e.sh runs on the host; fixed in 07cb7b9d6 (ulimit -c 0). r611 debug, us-east4-a: every check of every suite passed in the guest (51, 73, 73, 71); checks and overrun OK (77 of 421 s, 84 of 430 s; crashes and hangs as in r610, hangs 8.44 s of 4.50 s). -dax, -delay and -small failed on console KCSAN reports: fxfs_vfs_read_folios / lru_gen_add_folio, _find_next_bit (fxfs_dax_pages_would_dirty) / fxfs_dax_pages_dirty, __lruvec_stat_mod_folio / fxfs_vfs_page_mkwrite. r613 debug on the base origin/land-157 (490faad2c), the DAX suites: the same reports (-delay lru_gen_add_folio, -small pages_dirty twice), so land-157's, not this change's. The pages_dirty race is inside mmap_write, a write fault against write(2) on pages.c's dirty bitmap (TASK-160.4), unlocked in fxfs_dax_pages_weight. r612 release, us-west1-b, 07cb7b9d6: no core; checks, -delay, -small and overrun OK. -dax failed on stores_under_rebinds alone ('no page rebound in 2007041 rounds'), main's known regression, and the checks after it passed.

Structure: the class of bug, one check's crash or hang taking the rest and state leaking between checks, is gone: a check's process owns what it maps and opens, and one function owns resetting the rest.

Verified on main, 2026-11-05 (code of dde8d58d3; 752515020 adds backlog only): GCE r691, the Linux workspace, 2302 of 2303, the one failure TASK-187's a_store_after_a_workers_write_waits_for_one_lift (TASK-190; its fix 788e3b6b4 is on land-p, not main); GCE r690, the e2e suite (--tag e2e, 15 cases) jailed with the production seccomp filter, 15 of 15; CI 42137709782 on dde8d58d3 green (workspace 2303 of 2303, sim, shuttle, clippy scripts, guest, guest-sim); macOS FXDAX_TEST_PARTS=workspace scripts/test.sh at 752515020, 2118 of 2118; scripts/upstream-changes.py --check at 752515020 passes. AC #1: GCE r690's fxfs-guest-checks-overrun (dde8d58d3, release, jailed) passed: its console has 'FAIL: crashes (3.94ms of 3.00s): killed by signal 7 (Bus error)', 'crashes: left fxfs.dax_fail_maps at 1000, set back to 0', 'FAIL: hangs (6.12s of 3.00s): still running after its budget: killed', 'hangs: left the filesystem frozen, thawed', 51 checks PASS after them and 'FAILED: crashes, hangs'. AC #2: unknown_names_fail and run_to's leftovers_are_killed and overruns_are_killed pass in CI 42137709782's guest job.

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**Each guest check in a process of its own; the overrun suite runs the page cache's checks (TASK-160.30, 2026-11-05)**

The guest agent (PID 1) ran every check of fxfs-guest-checks under catch_unwind, so a crash took the VM and the checks after it. Decided: the agent runs itself again per check (`FXDAX_CHECK` names it), in a process group of its own, to its budget; past it the guest's stacks are dumped and the group killed. Then, in one place (`guest::run::Guest::put_back`), it sets fxfs's writable module parameters back to what the guest booted with, thaws the filesystem and mounts it again if a check left it so, saying each, before waiting 10 s at most for the group; a group that outlives that resets the VM, as a hang did before. The host's verifies are now per check, in the table, and run for the checks FXDAX_CHECKS names too, where they ran only for a whole suite. fxfs-guest-checks-overrun now runs two checks made to fail (a SIGBUS, a write into a frozen filesystem) and then the whole page cache suite, about 60 s where it took 4 s, to show the rest pass after them.
**Instead:** fork per check without exec (unsafe in a process with threads); the overrun suite with a few checks after the failing ones (cheaper, and shows less).
**Undoing it:** `run_to` back to calling the body in-process; the overrun suite's `Suites::Failing` checks dropped.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Each guest check runs in a process of its own to its budget, its state put back in one place (Guest::put_back); a crash or hang fails it by name and the rest run; unknown FXDAX_CHECKS names fail. On main: c6d21c763, 095a1b719, 8a804c8ea. Verified: GCE r690's overrun case (crashes and hangs fail by name, 51 pass) and the four checks suites 15 of 15; CI 42137709782's guest job.
<!-- SECTION:FINAL_SUMMARY:END -->
