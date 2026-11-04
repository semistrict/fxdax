---
id: TASK-137
title: >-
  VMM: a follower's Firecracker died of SIGSEGV under
  fxfs-guest-follow-torture-dax, by the default action, its handler never run
status: Done
assignee: []
created_date: '2026-10-31 07:09'
updated_date: '2026-11-04 10:40'
labels:
  - bug
  - followers
dependencies: []
priority: high
ordinal: 483000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Soak 2026-10-31 (scripts/soak-workloads-gce.sh replicas, FXDAX_FOLLOW_SECONDS=600, tree cc4f61cf0 plus the guest batch, Firecracker 412e0b8d4), round 5 of 5: 'follower2's VM exited signal: 11 (SIGSEGV) (core dumped)' about 5 minutes into the writer's 10, in the follower's reading; writer and follower1 ended OK, and no history checked. Firecracker registers a SIGSEGV handler that logs and exits, which did not run: core dumped means the kernel took the default action, as for a stack overflow or a bad stack pointer, the handler having no stack of its own (no SA_ONSTACK). The VMM's log ends a second before with a routine refused store by a kworker (KVM's asynchronous page fault). The core went with the host: test-linux-host.sh now keeps a crash's stacks in results/cores. Logs: /tmp/fxdax-soak-results/replicas/e2e/fxfs-guest-follow-torture-dax/round-5.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The crash's stacks are in hand, or 20 rounds at 10 minutes pass without it on the code it failed on
- [ ] #2 Its cause is fixed, with a test that fails before the fix
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31, r317 (11e5c7878): 8 of 8 rounds at 10 minutes, no crash; with the soak's 4 before it, 12 clean in 13.

2026-10-31, r321 (c44da93d0): 9 of 9, no crash: 17 clean since, 21 of 22 in all, r317 and r321 keeping any crash's stacks. Not cleared: at 1 in 22, 17 clean rounds come about half the time.

2026-10-31, r329: 9 of 9, no crash: 26 clean rounds at 10 minutes since the crash, 30 of 31 in all, Firecracker 412e0b8d4 throughout. Unexplained: nothing changed for it, and at this rate it could come back: results/cores keeps its stacks if it does. Left open for that.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Firecracker's fault handlers run on an alternate signal stack of 256 KiB (SA_ONSTACK) on every thread (vCPUs, API, gdb, the extent device's, main), installed before each thread's seccomp filter, and log the faulting thread, address and whether it overflowed its stack. test_stack_overflow_runs_the_handler fails with the stack taken out, the process killed by the signal as the follower's VMM was; the original crash's cause stays unknown until it recurs, when the log will say. Verified: macOS scripts/test.sh on the tree merged with batch 2026-11-04 (workspace 2,083/2,083, Shuttle, scripts); GitHub CI on the branch, every job (run 41806511048, Linux workspace with the codec's tests); GCE r540 in the jailer under the production seccomp filter (fxfs-guest-dax, checks-dax, crash-dax, migrate-tenant-dax, pgbench pass; the refusal gate's failure is TASK-167's, the jailer never mounting /proc).
<!-- SECTION:FINAL_SUMMARY:END -->
