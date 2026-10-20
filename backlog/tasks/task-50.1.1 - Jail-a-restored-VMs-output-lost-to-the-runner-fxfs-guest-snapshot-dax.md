---
id: TASK-50.1.1
title: 'Jail: a restored VM''s output lost to the runner (fxfs-guest-snapshot-dax)'
status: Done
assignee: []
created_date: '2026-10-20 12:12'
updated_date: '2026-10-20 14:27'
labels: []
dependencies: []
parent_task_id: TASK-50.1
priority: high
type: bug
ordinal: 157000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Under the real seccomp filter in the jailer (scripts/test-e2e.sh FXDAX_FIRECRACKER_SECCOMP=real FXDAX_FIRECRACKER_JAIL=1, GCE run real4), fxfs-guest-snapshot-dax failed: 'expected "OK a\nOK b\n", got ""', though the test's stdout.txt holds 'OK a\nOK b' and both restored VMs printed and exited. No call was refused. Passed in the jailer under the log filter (jail7). Suspect: the runner's capture of the test's output while the restored Firecrackers, which the jailer's change of user makes forget PR_SET_PDEATHSIG, or the jail's mount namespace, hold the test's pipes or files.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Reproduced by looping the case in the jailer (FXDAX_E2E_ROUNDS)
- [x] #2 Cause found and fixed; the case passes 20 rounds in a row in the jailer under the real filter
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The restore resets a guest's vsock connections by an event its kernel may take late, after the test's guest had reconnected (the jail's slower start widened the window): the new connection was reset (ENOTCONN on read, ECONNRESET on connect). The guest now retries on that reset, the host answers every connection (6147f4d). On GCE in the jailer under the real filter: migrate, snapshot, suspend 30 rounds each; whole suite 26/26.
<!-- SECTION:FINAL_SUMMARY:END -->
