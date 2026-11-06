---
id: TASK-187
title: >-
  fxfs-guest-crash-stray-dax fails on main: a stray DAX access no longer stops
  the VM with 159
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 17:15'
updated_date: '2026-11-06 02:22'
labels:
  - bug
  - dax
dependencies: []
priority: high
ordinal: 639000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-165.6's GCE runs and reproduced on main at 75bd872ba (GCE r623, round 2 of 20). The case (TASK-77 AC3) has the guest load or store at a DAX window offset nothing maps (fxfs.dax_stray); the window refuses it, the vCPU's KVM_RUN keeps failing with EFAULT, and Firecracker must exit 159 (FcExitCode::AccessRefused). Now either the VM process exits 0 (r620 round 1, r623 round 2: 'the VM process exited with 0, not 159', after 'KVM_RUN kept failing with EFAULT, ~27000 times in 1 s ... 1 private, 1 withheld'), or vCPU 0's KVM_RUN fails with EFAULT a dozen times at a time and then runs on, thousands of times, until the case's 15 s limit (r621 round 1, r622 round 2). In r620 and r623 the refused access was a kworker's (not a vCPU), the offset then restricted to reads. Likely since copy-on-write DAX (TASK-164.1, vendor/firecracker d53645971: writable DAX deleted, private pages, EFAULT handling). Results: /tmp/fxdax-r620..r623 on the machine that ran them; replay: FXDAX_WORKSPACE_TESTS=0 FXDAX_E2E_TEST=fxfs-guest-crash-stray-dax FXDAX_E2E_ROUNDS=20 scripts/test-gce.sh all.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The cause named, with the run that shows it
- [x] #2 fxfs-guest-crash-stray-dax passes 20 of 20 rounds on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Read r620-r623: the two failure modes (exit 0 after 'kept failing'; bursts of EFAULT then 'ran on' until the timeout).
2. Bisect on GCE: r640 13db5c67c (writable DAX), r641 37c331a6c (copy-on-write the only DAX), r642 13db5c67c with fxfs.dax_cow=1; seccomp off at all three (pread64 refused before 09b36991a).
3. Firecracker: Vcpu::exit sends its exit code before it signals exit_evt (upstream signalled first; the VMM could read no code and stop with Ok).
4. fxdax: the retry policy (Firecracker's Efaults) moves into fxdax as AccessFailures: a run of failures ends only after IN_A_ROW without one, not at the vCPU's next other exit; failed() lifts workers' restrictions itself. Firecracker uses it.
5. uffd: the pages restricted for workers' writes listed, a lift walking those, not every page of the window; restrict() logs withheld as withheld.
6. Tests: fxdax unit tests of AccessFailures (red with an exit ending the run); a Linux uffd test, a stray load and store through a real COW window, retried as the VMM does with exits between, given up on; Firecracker tests of the exit order and the give-up text, run on GCE by test-e2e.sh.
7. Verify: GCE stray-dax 20 rounds jailed and not; the other DAX suites jailed; macOS workspace+shuttle; Linux nextest -p fxdax + shuttle + vmm tests in Docker.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Cause (AC1). Two Firecracker faults, both older than copy-on-write DAX, which COW brought out:
1. Exit 0 (r620, r623): Vcpu::exit wrote exit_evt before sending VcpuResponse::Exited (upstream's order). The VMM thread, woken, drained the vCPUs' responses before the code was sent, found none and stopped with FcExitCode::Ok. r623's log shows the give-up ('KVM_RUN kept failing with EFAULT, 27617 times in 1s') followed by exit 0.
2. Timeout (r621, r622): Efaults::ran_on ended a run of EFAULTs at any KVM_RUN that didn't fail. vCPU 0, retrying the stray access, failed 2 to 57 times in under 1.5 ms, exited otherwise, failed again, 16,000 runs and more until the 15 s limit, never failing for 1 s straight. The likely exits are the guest's interrupt handlers (virtio-mmio's interrupt status, read on vCPU 0, which takes the devices' interrupts); vCPU 1, which doesn't, failed 1 s straight in r623.
When: COW. r641 at 37c331a6c (COW the only DAX) timed out in round 4 of 20 like r621; r643 at 13db5c67c with fxfs.dax_cow=1 timed out in round 4 the same way. Seccomp was off at r641 ('none') and logging at r643 ('log'), since pread64 was refused before 09b36991a. The control, 13db5c67c without COW 20 rounds, didn't run: r640 failed at once (13db5c67c's test-e2e.sh takes no FXDAX_FIRECRACKER_SECCOMP=none), and r644, its rerun with 'log', failed to start when gcloud's login expired. Why COW brings the exits in is not shown; the guest's writeback of private pages after the crash point, its device interrupts on vCPU 0, is the guess.
Fix:
- vendor/firecracker ada4501c3: announce_exit sends the code, then signals.
- e64c4b283 with fxdax 8a383c6b6: the policy is fxdax's AccessFailures (dax/failures.rs), one owner of when a VMM retries and gives up. A run ends only once a failure or an exit comes IN_A_ROW (10 ms) after its last failure; failed() asks for the lifts itself. Logs count the other exits among a run's failures.
Related, perf gate r632 (forked pgbench on DAX 0.57 of no-DAX; 2^18+ worker restrictions a VMM): each guest store to a page not here takes KVM's worker's write, a restriction to reads, the vCPU's EFAULT, a lift by the fault thread, and the store retried.
- 8079afca6: a lift walked every page of the window; it walks a list of the pages restricted.
- The next commit: guest_access_failed waits for the lifts it asked for, at most LIFT_PATIENCE (5 ms), where it returned at once and the vCPU spun on EFAULT.
- Measured by a_store_after_a_workers_write_waits_for_one_lift (Linux, 1 GiB window, 1,000 stores, Docker on OrbStack): main 357 failed retries a store, 6.1 ms a store; with the list 140, 408 us; with the wait 1.0, 199 us. The restriction count is unchanged, one per worker write: what changed is each one's cost. Not yet measured on GCE's forked pgbench.
Blind spots: Firecracker's vCPU code had no test that ran anywhere (test-e2e.sh ran extent:: alone, so efaults_given_up_on_tell_where never ran); the retry policy had no test; the guest case alone exercised a stray access, at random times. Now:
- dax::failures tests: red with an exit ending the run.
- a_stray_access_stays_refused_and_is_given_up_on: a real COW window over uffd, a worker's write then the vCPU's load and store retried as the VMM does, 13 exits between failures, given up on at 1 s of the test's clock.
- test_exit_code_sent_before_the_exit_event, in vmm, which test-e2e.sh now runs on GCE beside efaults_.
Looked around: no other place signals exit_evt or sends Exited.
Structure: the retry policy belonged with the window that makes memory inaccessible, not in the VMM; moved. Restricting for workers' writes at all is KVM's async page faults' doing (their worker writes for loads and stores alike), which TASK-170 left as it is. Nothing more fundamental found.

The lift's wait is 2cb1d4c42. Verified locally: macOS workspace 2,135 of 2,135 and Shuttle, before the lift's wait, which is Linux code alone. Linux in Docker (arm64, OrbStack) with it: fxdax 466 of 470, the 4 failures the container's (no git for builds_are_stamped_with_their_commit; a helper binary the 3 grants tests spawn); every Shuttle suite; vmm's 16 (extent::, efaults_, test_exit_code_). AC2 not met yet: gcloud's login expired. GCE runs still needed, on HEAD, numbers 645 to 649:
- fxfs-guest-crash-stray-dax, 20 rounds, unjailed and jailed;
- jailed fxfs-guest-dax, fxfs-guest-checks-dax, fxfs-guest-crash-dax and fxfs-guest-pgbench-dax-forked;
- the perf gate's forked-dax against r632's 0.573;
- the control, 13db5c67c without COW, 20 rounds, FXDAX_FIRECRACKER_SECCOMP=log, from worktree t187-13db5c67c.
Hosts fxdax-test-r645-t187, r646 and r648 were created before the login expired and were never used: delete them (scripts/test-gce.sh delete NAME) or let them go at their 8 h lifetime.

GCE on 2b63f36db (rebased onto ef8223900, TASK-172 merged cleanly):
- r645: fxfs-guest-crash-stray-dax 20 of 20, unjailed.
- r646: 20 of 20, jailed.
- r647, jailed: fxfs-guest-dax, fxfs-guest-checks-dax, fxfs-guest-crash-dax and fxfs-guest-pgbench-dax-forked pass. Their Firecracker logs have no EFAULT run, no give-up and no lift.
- r648, the control at 13db5c67c without COW (FXDAX_FIRECRACKER_SECCOMP=log): 20 of 20, so the failures came in with COW.
- r649, perf-gate-gce.sh single: every case passes; forked-dax 0.579 (dax-forked 823, 802, 816, 834, 870 against no-dax-forked median 1,421), still failing its 0.9, where r632 had 0.573.
The perf-gate lead is wrong as stated: the forked VMs' 2^18+ restrictions for workers' writes (r647, r649: 1,009 log lines each) came with no failed KVM_RUN at all. Restricting costs the vCPU nothing there, and this task's lift changes don't touch the forked gap; KVM's asynchronous page faults, a worker round trip for each page not yet here, remain a suspect.
Local runs on the rebased tree: macOS workspace 2,105 of 2,105 and Shuttle; Linux in Docker, fxdax 467 of 471 (the container's 4: no git, the grants tests' helper binary) and vmm 16 of 16. scripts/upstream-changes.py --check passes. The lift's wait is 94f404848. Firecracker's commits are on semistrict/firecracker branch fxdax-t187.

Checked on main, 2026-11-05 (code of dde8d58d3; 752515020 adds backlog only): GCE r690, the e2e suite jailed with the production seccomp filter, 15 of 15, fxfs-guest-crash-stray-dax and every DAX case among them; GCE r691, the Linux workspace, 2302 of 2303: a_stray_access_stays_refused_and_is_given_up_on passes, but this task's a_store_after_a_workers_write_waits_for_one_lift fails ('the worker's write went through': GCE's vm.max_map_count kept the test's zero pages installed, TASK-190). TASK-190's fix, 788e3b6b4, is on land-p, not main, so DoD #1 stays unchecked until it lands and a GCE workspace run passes. CI 42137709782 on dde8d58d3 green; macOS workspace 2118 of 2118 at 752515020.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A guest's stray DAX access is fatal again (exit 159). Two Firecracker faults that copy-on-write DAX brought out: the vCPU signalled its exit before it sent its code (exit 0), and any other exit ended a run of EFAULTs, so vCPU 0 never gave up (timeout). Fixed: Firecracker ada4501c3 (the code sent first); fxdax's AccessFailures, one owner of the retry policy, a run ending only after 10 ms with no failure (fa55ce5c8, vendor/firecracker 4c45f1345); a lift walking only the restricted pages (ee806303a) and a failing vCPU waiting up to 5 ms for its lift (9a8cb5bb6). Verified: stray-dax 20 of 20 unjailed (r645) and jailed (r646), the DAX suites jailed (r647), the control without COW 20 of 20 (r648); GCE r690 on main 15 of 15. Not done: DoD #1, r691's one failure is this task's a_store_after_a_workers_write_waits_for_one_lift, whose fix (TASK-190, 788e3b6b4) is not on main yet. The forked pgbench gap, 0.579 in r649, is not this retry loop (TASK-164.6).
<!-- SECTION:FINAL_SUMMARY:END -->
